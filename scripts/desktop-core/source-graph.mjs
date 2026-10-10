// Source receipts bind reviewed inputs; they are not platform cleanup evidence.
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { join } from 'node:path';
import { readFileSync } from 'node:fs';

export const DESKTOP_TARGETS = Object.freeze({
  linux: { goos: 'linux', goarch: 'amd64', cgo: '0', extras: ['with_gvisor', 'with_purego'] },
  win: { goos: 'windows', goarch: 'amd64', cgo: '0', extras: ['with_gvisor', 'with_purego'] },
  'mac-x64': { goos: 'darwin', goarch: 'amd64', cgo: '1', extras: ['with_gvisor'] },
  'mac-arm64': { goos: 'darwin', goarch: 'arm64', cgo: '1', extras: ['with_gvisor'] },
});
const SHARED_TAGS = ['badlinkname', 'tfogo_checklinkname0', 'with_acme', 'with_ccm',
  'with_clash_api', 'with_cloudflared', 'with_dhcp', 'with_naive_outbound', 'with_ocm',
  'with_openconnect', 'with_openvpn', 'with_quic', 'with_tailscale', 'with_usbip',
  'with_utls', 'with_wireguard'];
export const digest = (bytes) => createHash('sha256').update(bytes).digest('hex');
export function canonical(value) {
  if (Array.isArray(value)) return `[${value.map(canonical).join(',')}]`;
  if (value && typeof value === 'object') {
    return `{${Object.keys(value).sort().map((key) => `${JSON.stringify(key)}:${canonical(value[key])}`).join(',')}}`;
  }
  return JSON.stringify(value);
}
export function requireGraph(condition, message) {
  if (!condition) throw new Error(message);
}
export function verifyHash(file, expected) {
  requireGraph(/^[a-f0-9]{64}$/.test(expected ?? '') && digest(readFileSync(file)) === expected,
    `SHA-256 mismatch: ${file}`);
}
export const expectedTags = (key) => [...SHARED_TAGS, ...DESKTOP_TARGETS[key].extras].sort();
const same = (left, right) => canonical(left) === canonical(right);
const isSha = (value) => typeof value === 'string' && /^[a-f0-9]{64}$/.test(value);
const isTree = (value) => typeof value === 'string' && /^[a-f0-9]{40}$/.test(value);
const isModule = (value) => typeof value === 'string' && /^[a-zA-Z0-9._/-]+$/.test(value);

function isModulePartition(policy, inventory, absentField) {
  if (!policy || !same(Object.keys(policy).sort(), ['requiredLinked', absentField].sort())) return false;
  const required = policy.requiredLinked;
  const absent = policy[absentField];
  return Array.isArray(required) && Array.isArray(absent) && Array.isArray(inventory)
    && [...required, ...absent].every(isModule)
    && new Set([...required, ...absent]).size === required.length + absent.length
    && same([...required, ...absent].sort(), [...inventory].sort());
}

function hasPlatformModulePolicy(spec, key) {
  const platform = spec.platforms?.[key];
  return isModulePartition(platform?.patchedModules, spec.dependencyModules, 'allowedAbsent')
    // Every declared patched module must link on every target.
    && platform.patchedModules.allowedAbsent.length === 0
    && isModulePartition(platform.transportModules, Object.keys(spec.transportPins ?? {}), 'confirmedAbsent');
}

// A present desktop block is an explicit contract, including null. Only an
// absent key selects the historical graph, which mobile still owns unchanged.
export const hasDesktopSource = (manifest) => Object.hasOwn(manifest, 'desktopSourceBuild');
export function desktopSourceSpec(manifest) {
  if (!hasDesktopSource(manifest)) {
    requireGraph(manifest.sourceBuild?.sourceMode === undefined, 'Legacy sourceBuild cannot activate a desktop fork');
    return manifest.sourceBuild;
  }
  const spec = manifest.desktopSourceBuild;
  requireGraph(spec && typeof spec === 'object' && !Array.isArray(spec)
    && spec.sourceMode === 'fork-commit-v1'
    && typeof spec.upstreamVersion === 'string' && /^[0-9][0-9A-Za-z.+-]*$/.test(spec.upstreamVersion),
  'Explicit desktopSourceBuild object/fork mode/upstreamVersion required');
  return spec;
}
export const desktopSourceBaseline = (manifest) => hasDesktopSource(manifest)
  ? desktopSourceSpec(manifest).upstreamVersion : manifest.bundledCoreVersion;

// Activation belongs to the reviewed App manifest. No branch-head/default-URL fallback.
export function isForkSource(spec) {
  requireGraph(spec?.sourceMode === undefined || spec.sourceMode === 'fork-commit-v1', 'Unsupported desktop source mode');
  return spec?.sourceMode === 'fork-commit-v1';
}
export const desktopSourceManifestPath = (spec) => isForkSource(spec)
  ? 'scripts/desktop-core/source-manifest.json' : 'scripts/libbox-patches/source-manifest.json';
export function desktopSourceGoVersion(root, manifest) {
  const spec = validateSourcePins(manifest, 'linux', false);
  const path = join(root, desktopSourceManifestPath(spec));
  verifyHash(path, spec.sourceManifestSha256);
  const source = JSON.parse(readFileSync(path, 'utf8'));
  validateSourceManifest(source, spec);
  return source.goVersion;
}

// Output facts deliberately do not enter the pre-link BuildID: a CDHash
// contains the signed bytes, including that BuildID, and would form a cycle.
export function validateMacCodeSignature(signature, key) {
  requireGraph(Object.hasOwn(DESKTOP_TARGETS, key), 'Unknown signature platform');
  if (!key.startsWith('mac-')) {
    requireGraph(signature === undefined, 'Non-Mac receipt carries a Mac signature');
    return;
  }
  requireGraph(signature && same(Object.keys(signature).sort(), ['cdHash', 'state'])
    && (signature.state === 'signed' && typeof signature.cdHash === 'string'
      && /^[a-f0-9]{40}$/.test(signature.cdHash)
      || key === 'mac-x64' && signature.state === 'unsigned' && signature.cdHash === null),
  'Mac signature state/CDHash differs from platform policy');
}

export function observeMacCodeSignature(binary, key, run = spawnSync) {
  if (!key.startsWith('mac-')) return undefined;
  const inspect = (command, args) => {
    let result;
    try { result = run(command, args, { encoding: 'utf8', stdio: 'pipe' }); }
    catch (error) { result = { status: error.status, stdout: error.stdout, stderr: error.stderr, error }; }
    if (typeof result === 'string' || Buffer.isBuffer(result)) return { status: 0, stdout: String(result), stderr: '' };
    requireGraph(result && Number.isInteger(result.status), `Cannot inspect Mac signature: ${command}`);
    return { status: result.status, stdout: String(result.stdout ?? ''), stderr: String(result.stderr ?? '') };
  };
  const verified = inspect('codesign', ['--verify', '--strict', binary]);
  let signature;
  if (verified.status === 0) {
    const display = inspect('codesign', ['--display', '--verbose=4', binary]);
    const matches = [...(display.stdout + '\n' + display.stderr).matchAll(/^CDHash=([a-fA-F0-9]{40})\r?$/gm)];
    requireGraph(display.status === 0 && matches.length === 1, 'Signed Mac core has no unique actual CDHash');
    signature = { state: 'signed', cdHash: matches[0][1].toLowerCase() };
  } else {
    requireGraph(key === 'mac-x64' && verified.status === 1
      && verified.stderr.includes('code object is not signed at all'), 'Mac core signature invalid or missing');
    const kind = inspect('file', ['-b', binary]);
    requireGraph(kind.status === 0 && /^Mach-O\b.*\bx86_64\b/.test(kind.stdout.trim())
      && !/arm64|universal|fat/i.test(kind.stdout), 'Unsigned exception requires a thin Intel Mach-O');
    signature = { state: 'unsigned', cdHash: null };
  }
  validateMacCodeSignature(signature, key);
  return signature;
}

export function verifyMacCodeSignatureReceipt(binary, receipt, key, run) {
  requireGraph(key === 'mac-x64' || key === 'mac-arm64', 'Mac signature receipt requires a Mac target');
  validateMacCodeSignature(receipt.macCodeSignature, key);
  const { fingerprint, ...facts } = receipt;
  requireGraph(receipt.schema === 'polaris-desktop-core-v1' && receipt.platform === key
    && fingerprint === digest(canonical(facts)), 'Mac signature receipt fingerprint/platform differs');
  verifyHash(binary, receipt.binarySha256);
  requireGraph(same(observeMacCodeSignature(binary, key, run), receipt.macCodeSignature),
    'Actual Mac signature state/CDHash differs from receipt');
  verifyHash(binary, receipt.binarySha256);
}

export const desktopOverlays = (spec, key, overlaySha256) => isForkSource(spec) || key !== 'win'
  ? [] : [{ file: 'windows-dns-refresh.patch', sha256: overlaySha256 }];

export function validateSourcePins(manifest, key, outputsRequired = true) {
  requireGraph(Object.hasOwn(DESKTOP_TARGETS, key), `Unknown desktop core target: ${key}`);
  const spec = desktopSourceSpec(manifest);
  const baseline = desktopSourceBaseline(manifest);
  requireGraph(!isForkSource(spec) || manifest.windowsBuild === undefined, 'Fork source cannot retain an App Windows overlay contract');
  const frozen = spec && ['sourceManifestSha256', 'provisionerSha256', 'sourceReceiptFingerprint',
    'moduleGraphSha256'].every((field) => isSha(spec[field]))
    && isTree(spec.patchedSourceTree) && isTree(spec.buildTree)
    && Array.isArray(spec.dependencyModules)
    && spec.dependencyModules.every(isModule)
    && new Set(spec.dependencyModules).size === spec.dependencyModules.length
    && spec.transportPins && typeof spec.transportPins === 'object' && !Array.isArray(spec.transportPins)
    && Object.keys(spec.transportPins).length > 0 && Object.entries(spec.transportPins).every(([module, version]) =>
      isModule(module) && typeof version === 'string' && /^v[0-9A-Za-z.+-]+$/.test(version))
    && typeof spec.version === 'string' && spec.version.startsWith(`${baseline}.polaris.`)
    && /^[1-9][0-9]*$/.test(spec.version.slice(`${baseline}.polaris.`.length))
    && same(Object.keys(spec.platforms ?? {}).sort(), Object.keys(DESKTOP_TARGETS).sort())
    && Object.entries(spec.platforms).every(([platformKey, platform]) => isTree(platform?.buildTree)
      && hasPlatformModulePolicy(spec, platformKey)
      && (!outputsRequired || isSha(platform.binarySha256)))
    && (!hasDesktopSource(manifest) || isSha(spec.mainGoModSha256) && isSha(spec.mainGoSumSha256)
      && spec.graphScope === 'core-source-only' && spec.dependencyModules.length === 0
      && spec.buildTree === spec.patchedSourceTree
      && Object.values(spec.platforms).every((platform) => platform.buildTree === spec.buildTree));
  requireGraph(frozen, `Desktop source graph is not frozen: combined source/dependency/receipt${outputsRequired ? ' and four output' : ' input'} pins are required; official assets and old cached cores cannot substitute`);
  return spec;
}

export function frozenSourceVersion(manifest) {
  try { return validateSourcePins(manifest, 'linux', false).version; }
  catch { return undefined; } // Runtime baseline compatibility; producers reject.
}

export function platformSourceIdentity(receipt, source, spec, key, overlaySha256) {
  const target = DESKTOP_TARGETS[key];
  const facts = { sourceReceiptFingerprint: receipt.fingerprint, buildTree: spec.platforms[key].buildTree,
    overlays: desktopOverlays(spec, key, overlaySha256),
    version: spec.version, goVersion: source.goVersion, platform: key, transportPins: spec.transportPins,
    patchedModules: spec.platforms[key].patchedModules, transportModules: spec.platforms[key].transportModules,
    goos: target.goos, goarch: target.goarch, cgo: target.cgo, tags: expectedTags(key) };
  const platformInputFingerprint = digest(canonical(facts));
  return { sourceFingerprint: receipt.fingerprint, platformInputFingerprint,
    buildID: `polaris-desktop-source-v1-${platformInputFingerprint}` };
}

export function validateSourceManifest(source, spec) {
  requireGraph(isTree(source.sourceCommit) && /^\d+\.\d+\.\d+$/.test(source.goVersion ?? ''), 'Invalid source/toolchain pin');
  if (isForkSource(spec)) {
    requireGraph(source.schema === 'polaris-desktop-fork-source-v1' && source.role === 'desktop'
      && source.sourceURL === 'https://github.com/polaris-arch/polaris-box', 'Desktop fork repository/schema/role differs');
    const version = /^(.*)\.polaris\.[1-9][0-9]*$/.exec(spec.version ?? '')?.[1];
    requireGraph(version && version === spec.upstreamVersion && source.upstreamTag === `v${version}` && isTree(source.upstreamCommit)
      && typeof source.sourceTag === 'string' && source.sourceTag.startsWith(`polaris-${source.upstreamTag}-`)
      && /^[1-9][0-9]*$/.test(source.sourceTag.slice(`polaris-${source.upstreamTag}-`.length))
      && isTree(source.sourceTagObject), 'Immutable fork tag/upstream pins missing or inconsistent');
    requireGraph(isTree(source.sourceTree) && source.sourceTree === spec.patchedSourceTree
      && source.sourceTree === spec.buildTree
      && Object.values(spec.platforms ?? {}).every((platform) => platform.buildTree === source.sourceTree),
    'Desktop fork commit tree must be shared by all four targets; mobile/overlay trees cannot substitute');
    requireGraph(Array.isArray(source.patches) && source.patches.length === 0
      && Array.isArray(source.dependencyPatches) && source.dependencyPatches.length === 0
      && Array.isArray(spec.dependencyModules) && spec.dependencyModules.length === 0,
    'Fork source must not replay App patches or dependency overlays');
    return [];
  }
  requireGraph(source.schema === undefined && source.role === undefined && source.sourceTag === undefined
    && source.sourceTagObject === undefined && source.sourceTree === undefined && source.upstreamTag === undefined
    && source.upstreamCommit === undefined
    && (source.sourceURL === undefined || source.sourceURL === 'https://github.com/SagerNet/sing-box'),
  'Fork inputs require explicit desktop fork source mode');
  requireGraph(Array.isArray(source.patches) && source.patches.length > 0 && source.patches.every((patch) =>
    /^[a-z0-9-]+\.patch$/.test(patch.file ?? '') && isSha(patch.sha256)), 'Invalid core patch inventory');
  requireGraph(new Set(source.patches.map((patch) => patch.file)).size === source.patches.length, 'Duplicate core patch');
  const dependencies = source.dependencyPatches;
  requireGraph(Array.isArray(dependencies), 'Patched dependency graph is not frozen');
  requireGraph(same(dependencies.map((dep) => dep.module).sort(), [...spec.dependencyModules].sort()), 'Patched module inventory differs');
  requireGraph(new Set(dependencies.map((dep) => dep.name)).size === dependencies.length, 'Duplicate dependency directory');
  for (const dep of dependencies) {
    requireGraph(/^[a-z0-9][a-z0-9-]*$/.test(dep.name ?? '') && /^v\S+$/.test(dep.upstreamVersion ?? '')
      && isTree(dep.upstreamCommit) && isTree(dep.patchedTree) && isSha(dep.patchSha256)
      && /^[a-z0-9-]+\.patch$/.test(dep.patchFile ?? '')
      && /^https:\/\/github\.com\/[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+(?:\.git)?$/.test(dep.sourceURL ?? ''), 'Invalid dependency source pin');
  }
  return dependencies;
}

export function validateSourceReceipt(receipt, source, spec) {
  validateSourceManifest(source, spec);
  requireGraph(receipt?.schema === 'polaris-core-source-v1', 'Unsupported source receipt schema');
  if (isForkSource(spec)) requireGraph(receipt.upstreamTree === source.sourceTree
    && receipt.mainGoModSha256 === spec.mainGoModSha256 && receipt.mainGoSumSha256 === spec.mainGoSumSha256,
  'Fork receipt commit tree/full main manifests differ');
  const [graphScope, sourceGraphState] = source.dependencyPatches.length > 0
    ? ['declared-patched-modules', 'dependencies-patched'] : ['core-source-only', 'source-only'];
  requireGraph(receipt.graphScope === graphScope && receipt.sourceGraphState === sourceGraphState,
    'Unsupported/unfrozen module graph scope; not a full linked graph proof');
  const queries = source.dependencyPatches.map((dep) => dep.module).sort();
  requireGraph(same(receipt.moduleGraphQueries, queries)
    && Array.isArray(receipt.moduleGraph) && same(receipt.moduleGraph.map((module) => module.Path).sort(), queries),
  'Shared provider declared module query scope differs');
  requireGraph(isSha(receipt.mainGoModSha256) && isSha(receipt.mainGoSumSha256), 'Full main go.mod/go.sum bindings missing');
  const { fingerprint, ...facts } = receipt;
  requireGraph(isSha(fingerprint) && digest(canonical(facts)) === fingerprint
    && fingerprint === spec.sourceReceiptFingerprint, 'Source receipt fingerprint differs');
  requireGraph(receipt.sourceCommit === source.sourceCommit && isTree(receipt.upstreamTree)
    && receipt.sourceURL === (source.sourceURL ?? 'https://github.com/SagerNet/sing-box')
    && receipt.patchedSourceTree === spec.patchedSourceTree && receipt.buildTree === spec.buildTree
    && receipt.sourceManifestSha256 === spec.sourceManifestSha256
    && receipt.provisionerSha256 === spec.provisionerSha256
    && same(receipt.patches, source.patches), 'Source receipt input binding differs');
  requireGraph(Array.isArray(receipt.dependencies) && receipt.dependencies.length === source.dependencyPatches.length,
    'Source receipt dependency inventory differs');
  for (const [index, dep] of source.dependencyPatches.entries()) {
    const actual = receipt.dependencies[index];
    requireGraph(actual && Object.entries(dep).every(([field, value]) => same(actual[field], value))
      && isTree(actual.upstreamTree) && actual.replacement === `./polaris-dependencies/${dep.name}`,
    'Source receipt dependency binding differs');
    const modules = receipt.moduleGraph?.filter((module) => module.Path === dep.module);
    requireGraph(modules?.length === 1 && modules[0].Version === dep.upstreamVersion
      && modules[0].Replace?.Path === actual.replacement, 'Source receipt module replacement differs');
  }
  requireGraph(Array.isArray(receipt.moduleGraph)
    && digest(canonical(receipt.moduleGraph)) === receipt.moduleGraphSha256
    && receipt.moduleGraphSha256 === spec.moduleGraphSha256, 'Source receipt module graph fingerprint differs');
  return receipt;
}

// `go version -m` reads build information without executing the target binary.
// Local replacements use the following => row; a version alone is insufficient.
export function parseBuildInfo(raw) {
  const lines = String(raw).trimEnd().split(/\r?\n/);
  const goVersion = /: go(\d+\.\d+\.\d+)$/.exec(lines.shift() ?? '')?.[1];
  requireGraph(goVersion, 'Missing native Go build information');
  const modules = new Map();
  const settings = new Map();
  let previous;
  for (const line of lines) {
    // Go prints a tab-only separator after a local replacement block.
    if (line === '\t') { previous = undefined; continue; }
    const parts = line.split('\t');
    requireGraph(parts[0] === '', 'Malformed native Go build information');
    const [, kind, value, version] = parts;
    if (kind === 'dep' || kind === 'mod') {
      requireGraph(value && version && !modules.has(value), 'Duplicate/malformed embedded module');
      previous = { kind, version, ...(parts[4] ? { sum: parts[4] } : {}) };
      modules.set(value, previous);
    } else if (kind === '=>') {
      requireGraph(previous && value && !previous.replacement, 'Unbound/duplicate embedded replacement');
      previous.replacement = value;
      if (version) previous.replacementVersion = version;
      if (parts[4]) previous.replacementSum = parts[4];
      previous = undefined;
    } else if (kind === 'build') {
      const split = value?.indexOf('=') ?? -1;
      requireGraph(split > 0, 'Malformed embedded build setting');
      const key = value.slice(0, split);
      requireGraph(!settings.has(key), 'Duplicate embedded build setting');
      let setting = value.slice(split + 1);
      if (setting.startsWith('"')) setting = JSON.parse(setting);
      settings.set(key, setting);
      previous = undefined;
    } else if (kind !== 'path') throw new Error('Unsupported native Go build information row');
  }
  return { goVersion, modules, settings };
}

export function validateBuildInfo(raw, source, key, spec) {
  requireGraph(Object.hasOwn(DESKTOP_TARGETS, key) && spec && hasPlatformModulePolicy(spec, key),
    'Platform linked module policy differs');
  requireGraph(same(source.dependencyPatches.map((dep) => dep.module).sort(), [...spec.dependencyModules].sort()),
    'Patched module inventory differs');
  const facts = parseBuildInfo(raw);
  const target = DESKTOP_TARGETS[key];
  const platform = spec.platforms[key];
  requireGraph(facts.goVersion === source.goVersion, 'Embedded Go toolchain differs');
  for (const [field, expected] of [['GOOS', target.goos], ['GOARCH', target.goarch], ['CGO_ENABLED', target.cgo]]) {
    requireGraph(facts.settings.get(field) === expected, `Embedded ${field} differs`);
  }
  requireGraph(same((facts.settings.get('-tags') ?? '').split(',').sort(), expectedTags(key)), 'Embedded feature tags differ');
  for (const dep of source.dependencyPatches) {
    const actual = facts.modules.get(dep.module);
    if (!actual && platform.patchedModules.allowedAbsent.includes(dep.module)) continue;
    requireGraph(actual?.version === dep.upstreamVersion
      && actual.replacement === `./polaris-dependencies/${dep.name}`, `Embedded patched dependency differs: ${dep.module}`);
  }
  for (const [module, version] of Object.entries(spec.transportPins)) {
    const actual = facts.modules.get(module);
    if (platform.transportModules.confirmedAbsent.includes(module)) {
      requireGraph(!actual, `Unexpected embedded transport module: ${module}`);
      continue;
    }
    requireGraph(actual?.version === version, `Embedded transport module version differs: ${module}`);
    if (!source.dependencyPatches.some((dep) => dep.module === module)) {
      requireGraph(!actual.replacement, `Unreviewed embedded transport replacement: ${module}`);
    }
  }
  // Any linked module carrying a replacement row must be a declared dependency.
  for (const [module, actual] of facts.modules) {
    requireGraph(!actual.replacement || spec.dependencyModules.includes(module), `Undeclared embedded module replacement: ${module}`);
  }
  return facts;
}

// The go-version header names the inspected file. Artifact transport changes
// that path; fingerprint parsed facts rather than an absolute build directory.
export function buildInfoFingerprint(raw) {
  const facts = parseBuildInfo(raw);
  return digest(canonical({ goVersion: facts.goVersion,
    modules: [...facts.modules.entries()].sort(([a], [b]) => a.localeCompare(b)),
    settings: [...facts.settings.entries()].sort(([a], [b]) => a.localeCompare(b)) }));
}

// Section-local corresponding-source guidance cannot be supplied by another
// platform's text. Legacy NOTICE retains its existing policy in the CLI.
export function validateForkNotice(notice, manifest, source, mobile) {
  const spec = validateSourcePins(manifest, 'linux', false);
  validateSourceManifest(source, spec);
  const block = (label) => {
    const lines = notice.split(/\r?\n/);
    const starts = lines.flatMap((line, index) => line.startsWith(`  - ${label} `) ? [index] : []);
    requireGraph(starts.length === 1, `NOTICE: unique ${label} section required`);
    const end = lines.findIndex((line, index) => index > starts[0] && line.startsWith('  - '));
    return lines.slice(starts[0], end < 0 ? lines.length : end).map((line) => line.trim());
  };
  const exact = (lines, expected) => {
    const label = expected.slice(0, expected.indexOf('：') + 1);
    requireGraph(lines.filter((line) => line.startsWith(label)).length === 1 && lines.includes(expected),
      `NOTICE: section-local identity missing/duplicated: ${expected}`);
  };
  const desktop = block('桌面 sing-box');
  requireGraph(desktop[0] === `- 桌面 sing-box v${spec.upstreamVersion} (GPLv3) — ${source.sourceURL}`,
    'NOTICE: desktop baseline/repository differs');
  for (const line of [`上游基线源码：https://github.com/SagerNet/sing-box/tree/${source.upstreamTag}`,
    `桌面源码修复构建：${spec.version}`, `固定源码：${source.sourceURL}/tree/${source.sourceCommit}`,
    `固定消费标签：${source.sourceURL}/tree/${source.sourceTag}`, `消费标签对象：${source.sourceTagObject}`,
    `共同 source manifest SHA-256：${spec.sourceManifestSha256}`]) exact(desktop, line);
  for (const [label, identity] of [['Android libbox 基线', mobile.android], ['Apple libbox 基线', mobile.apple]]) {
    requireGraph(isTree(identity?.sourceCommit) && isSha(identity.manifestSha256), 'NOTICE: mobile source identity missing');
    const lines = block(label);
    requireGraph(lines[0] === `- ${label} v${manifest.bundledCoreVersion} (GPLv3) — https://github.com/SagerNet/sing-box`,
      `NOTICE: ${label} baseline differs`);
    for (const line of [`上游基线源码：https://github.com/SagerNet/sing-box/tree/v${manifest.bundledCoreVersion}`,
      `固定源码：https://github.com/SagerNet/sing-box/tree/${identity.sourceCommit}`,
      `source manifest SHA-256：${identity.manifestSha256}`]) exact(lines, line);
  }
  const heading = '════ 对应源码 / Corresponding Source ════';
  requireGraph(notice.split(heading).length === 2, 'NOTICE: unique corresponding-source guidance required');
  const guidance = notice.split(heading)[1];
  requireGraph(!/Windows\s*另加|固定\s*Windows\s*附加补丁目录|Windows\s+additionally\s+uses|fixed\s+Windows\s+overlay\s+directory/i.test(guidance),
    'NOTICE: retired Windows overlay claim conflicts with fixed fork source');
  const patchBase = 'https://github.com/polaris-arch/Polaris/tree/4563b1fc1ecb23cb7f1141a37a2cab144891ecd6/scripts';
  const lines = guidance.split(/\r?\n/).map((line) => line.trim());
  for (const line of [`桌面 D 完整对应源码：${source.sourceURL}/tree/${source.sourceCommit}`,
    `桌面 D 源码资产：${source.sourceURL}/releases/tag/${source.sourceTag}`,
    `Android 旧补丁目录：${patchBase}/libbox-patches`,
    `Apple 旧补丁目录：${patchBase}/libbox-ios-patches`]) exact(lines, line);
}
