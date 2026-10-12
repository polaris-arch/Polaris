import { createHash, randomUUID } from 'node:crypto';
import { lstatSync, openSync, closeSync, readSync, readFileSync, fstatSync, constants, writeFileSync, renameSync, rmSync, existsSync } from 'node:fs';
import { join } from 'node:path';

const hash = (bytes) => createHash('sha256').update(bytes).digest('hex');
const hex = (value) => typeof value === 'string' && /^[0-9a-f]{64}$/.test(value);
function demand(value, reason) { if (!value) throw new Error(`Core identity: ${reason}`); }
function u64(value) { demand(Number.isSafeInteger(value) && value >= 0, 'invalid integer'); const bytes = Buffer.alloc(8); bytes.writeBigUInt64BE(BigInt(value)); return bytes; }
function field(value) { const bytes = Buffer.from(value, 'utf8'); return Buffer.concat([u64(bytes.length), bytes]); }

/** Read-only final packaging check; the source manifest is the compiled input. */
export function verifySelectedCoreManifest(directory, sourceDirectory, platform) {
  const finalPath = join(directory, 'core-manifest.json'), sourcePath = join(sourceDirectory, 'core-manifest.json');
  for (const path of [finalPath, sourcePath]) {
    const stat = lstatSync(path); demand(stat.isFile() && !stat.isSymbolicLink() && stat.nlink === 1 && stat.size <= 64 * 1024, 'invalid installation manifest object');
  }
  const source = readFileSync(sourcePath), final = readFileSync(finalPath);
  demand(source.equals(final), 'final manifest differs from compiled installation input');
  const value = JSON.parse(source);
  demand(value?.schema === 'polaris-core-manifest-v1' && value.core?.platform === platform
    && hex(value.id) && value.id === hash(canonicalCoreIdentity(value.core)), 'installation identity differs');
  const helper = platform === 'win' ? 'polaris-helper.exe' : 'polaris-helper';
  demand(hex(value.core.helperSha256), 'actual helper pin required');
  demand(actualCoreFile(join(directory, helper)).sha256 === value.core.helperSha256, 'final helper bytes differ from compiled pin');
  for (const file of value.core.files) actualCoreFile(join(directory, file.name), file);
  if (platform === 'win') {
    demand(hex(value.core.cleanerSha256), 'actual cleaner pin required');
    demand(actualCoreFile(join(directory, 'polaris-cleaner.exe')).sha256 === value.core.cleanerSha256, 'final cleaner bytes differ from compiled pin');
  } else demand(value.core.cleanerSha256 === null || value.core.cleanerSha256 === undefined, 'unexpected cleaner');
  return value;
}

export function canonicalCoreIdentity(core) {
  const names = { linux: ['sing-box', 'libcronet.so'], win: ['sing-box.exe', 'libcronet.dll'],
    'mac-x64': ['sing-box'], 'mac-arm64': ['sing-box'] }[core.platform];
  demand(names && typeof core.version === 'string' && /^[A-Za-z0-9.+-]{1,128}$/.test(core.version)
    && hex(core.providerFingerprint), 'invalid platform/version/provider');
  demand(Array.isArray(core.files) && core.files.length === names.length
    && new Set(core.files.map((file) => file.name)).size === names.length, 'incomplete file set');
  const files = [...core.files].sort((a, b) => Buffer.compare(Buffer.from(a.name), Buffer.from(b.name)));
  for (const file of files) demand(names.includes(file.name) && hex(file.sha256)
    && Number.isSafeInteger(file.bytes) && file.bytes > 0 && file.bytes <= 512 * 1024 ** 2, 'invalid file');
  return Buffer.concat([Buffer.from('polaris-core-manifest-v1\0'), field(core.platform), field(core.version),
    field(core.providerFingerprint), u64(files.length), ...files.flatMap((file) => [field(file.name), u64(file.bytes), field(file.sha256)])]);
}

export function coreIdentityId(core) { return hash(canonicalCoreIdentity(core)); }

export function selectedCoreManifest(contract, platform, appVersion, helperSha256 = null, cleanerSha256 = null) {
  const row = contract.platforms[platform]; demand(row, 'missing selected platform');
  const entry = (file) => ({ name: file.member, bytes: file.bytes, sha256: file.sha256 });
  const core = { platform, version: contract.version, providerFingerprint: row.providerFingerprint,
    files: [entry(row.core), ...(row.sidecar ? [entry(row.sidecar)] : [])], helperSha256, cleanerSha256 };
  demand(helperSha256 === null || hex(helperSha256), 'invalid helper hash');
  demand(cleanerSha256 === null || platform === 'win' && hex(cleanerSha256), 'invalid cleaner hash');
  demand(typeof appVersion === 'string' && /^[A-Za-z0-9.+-]{1,128}$/.test(appVersion), 'invalid App version');
  return { schema: 'polaris-core-manifest-v1', id: coreIdentityId(core), appVersion, core };
}

export function actualCoreFile(path, expected) {
  const before = lstatSync(path); demand(before.isFile() && !before.isSymbolicLink() && before.nlink === 1, 'regular single-link object required');
  const fd = openSync(path, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0) | (constants.O_NONBLOCK ?? 0));
  try {
    const opened = fstatSync(fd); demand(opened.isFile() && opened.ino === before.ino && opened.dev === before.dev, 'object replaced');
    const digest = createHash('sha256'); const buffer = Buffer.alloc(64 * 1024); let bytes = 0, n;
    while ((n = readSync(fd, buffer, 0, buffer.length, null)) > 0) {
      bytes += n; demand(bytes <= 512 * 1024 ** 2, 'object too large'); digest.update(buffer.subarray(0, n));
    }
    const after = fstatSync(fd); demand(bytes === opened.size && after.size === opened.size && after.mtimeMs === opened.mtimeMs, 'object changed');
    const sha256 = digest.digest('hex');
    if (expected) demand(bytes === expected.bytes && sha256 === expected.sha256, 'selected bytes differ');
    return { bytes, sha256 };
  } finally { closeSync(fd); }
}

/** Packaging-stage generation only; never guesses helper or post-transform byte hashes. */
export function writeSelectedCoreManifest(directory, contract, platform, appVersion) {
  const dir = lstatSync(directory); demand(dir.isDirectory() && !dir.isSymbolicLink(), 'payload directory required');
  const helper = platform === 'win' ? 'polaris-helper.exe' : 'polaris-helper';
  const helperSha256 = actualCoreFile(join(directory, helper)).sha256;
  const cleanerSha256 = platform === 'win' ? actualCoreFile(join(directory, 'polaris-cleaner.exe')).sha256 : null;
  const manifest = selectedCoreManifest(contract, platform, appVersion, helperSha256, cleanerSha256);
  for (const expected of manifest.core.files) actualCoreFile(join(directory, expected.name), expected);
  writeManifest(directory, manifest);
  return manifest;
}

function writeManifest(directory, manifest) {
  const destination = join(directory, 'core-manifest.json');
  if (existsSync(destination)) demand(lstatSync(destination).isFile() && !lstatSync(destination).isSymbolicLink(), 'manifest object is not regular');
  const temporary = join(directory, `.core-manifest-${randomUUID()}.json`);
  try {
    writeFileSync(temporary, JSON.stringify(manifest, null, 2) + '\n', { flag: 'wx', mode: 0o644 });
    renameSync(temporary, destination);
  } finally { rmSync(temporary, { force: true }); }
}
