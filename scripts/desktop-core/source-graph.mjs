// Shared helpers for the committed desktop release consumer.
import { createHash } from 'node:crypto';
import { spawnSync } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { loadReleaseContract } from './release-consumer.mjs';

// Hosting identity only. These records do not select a release or assert that
// artifacts exist.
export const KERNEL_REPOSITORIES = Object.freeze({
  'polaris-arch/sing-box-polaris': Object.freeze({ id: 1421657509, branch: 'polaris-main' }),
});
export function kernelRepositoryIdentity(repository) {
  requireGraph(Object.hasOwn(KERNEL_REPOSITORIES, repository), 'Unknown kernel repository');
  return KERNEL_REPOSITORIES[repository];
}

export const DESKTOP_TARGETS = Object.freeze({
  linux: { goos: 'linux', goarch: 'amd64', cgo: '0', extras: ['with_gvisor', 'with_purego'] },
  win: { goos: 'windows', goarch: 'amd64', cgo: '0', extras: ['with_gvisor', 'with_purego'] },
  'mac-x64': { goos: 'darwin', goarch: 'amd64', cgo: '1', extras: ['with_gvisor'] },
  'mac-arm64': { goos: 'darwin', goarch: 'arm64', cgo: '1', extras: ['with_gvisor'] },
});
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
const same = (left, right) => canonical(left) === canonical(right);

// The Go toolchain is the one recorded by the committed release contract.
export function releaseGoVersion(contract) {
  const version = contract?.sourcePolicy?.goVersion;
  requireGraph(typeof version === 'string' && /^1\.[0-9]+\.[0-9]+$/.test(version), 'Release Go version missing');
  return `go${version}`;
}

export function desktopSourceGoVersion(root, manifest) {
  const contract = loadReleaseContract(root, manifest);
  const goVersions = new Set(Object.values(contract.platforms).map((row) => row.goVersion));
  requireGraph(goVersions.size === 1 && goVersions.has(releaseGoVersion(contract)), 'Release inspection Go pin differs');
  return releaseGoVersion(contract).slice(2);
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

const MAC_SIGNATURE_TIMEOUT_MS = 60_000;

export function observeMacCodeSignature(binary, key, run = spawnSync) {
  if (!key.startsWith('mac-')) return undefined;
  const inspect = (command, args) => {
    let result;
    try { result = run(command, args, { encoding: 'utf8', stdio: 'pipe',
      timeout: MAC_SIGNATURE_TIMEOUT_MS, killSignal: 'SIGKILL' }); }
    catch (error) { result = { status: error.status, stdout: error.stdout, stderr: error.stderr, error }; }
    if (typeof result === 'string' || Buffer.isBuffer(result)) return { status: 0, stdout: String(result), stderr: '' };
    requireGraph(result?.error?.code !== 'ETIMEDOUT', `Mac signature command timed out: ${command} ${args[0]}`);
    requireGraph(result && Number.isInteger(result.status),
      `Cannot inspect Mac signature: ${command} ${args[0]} (${result?.error?.code ?? 'no exit status'})`);
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
