// Explicit release acquisition for build resources. Never executes target code.
import { rejectLocalDistribution } from './local-development.mjs';
import { execFileSync, spawnSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { homedir } from 'node:os';
import { chmodSync, closeSync, copyFileSync, cpSync, existsSync, fsyncSync, lstatSync, mkdirSync, mkdtempSync,
  openSync, readFileSync, readdirSync, renameSync, rmSync, writeFileSync, constants, fstatSync,
  opendirSync, readSync, realpathSync, rmdirSync, statfsSync, unlinkSync, writeSync } from 'node:fs';
import { basename, dirname, join, resolve, sep } from 'node:path';
import { fileURLToPath } from 'node:url';
import { canonical, digest, observeMacCodeSignature, validateMacCodeSignature,
  kernelRepositoryIdentity, requireGraph as demand } from './source-graph.mjs';
import { isUpstreamRelease, validateUpstreamContract, validateUpstreamRow,
  upstreamMetadata, upstreamPayloadPins, contractPublicAssets } from './upstream-release.mjs';
import { inspectCronetBytes } from '../lib/cronet-payload.mjs';

const REPO = 'polaris-arch/sing-box-polaris';
export function releaseRepositoryId(contract) {
  return kernelRepositoryIdentity(contract.repository).id;
}
// Only consumer steps explicitly receive the built-in Actions token. Keep it
// in this reader's closure, not in the environment inherited by native tools.
const RELEASE_API_ENV = Object.freeze({ GITHUB_ACTIONS: process.env.GITHUB_ACTIONS,
  POLARIS_RELEASE_API_TOKEN: process.env.POLARIS_RELEASE_API_TOKEN });
delete process.env.POLARIS_RELEASE_API_TOKEN;
const POLICY = 'preserve measured original bytes; ad-hoc/not-signed observations do not assert trust';
// Local inspection must fail closed before the outer gate deadline; no retries or target execution.
const LOCAL_METADATA_TIMEOUT_MS = 60_000;
const LOCAL_ARCHIVE_TIMEOUT_MS = 120_000;
// Cold acquisition stays inside consumeRelease's acquiring transaction. Child
// failures must return to its finally, without logging input, paths or raw errors.
function runColdArchive(run, stage, args, input) {
  const started = performance.now();
  try {
    return run(process.platform === 'win32' ? 'python' : 'python3', args,
      { input, encoding: 'utf8', maxBuffer: 1024 * 1024,
        timeout: LOCAL_ARCHIVE_TIMEOUT_MS, killSignal: 'SIGKILL' });
  } catch (error) {
    const code = ['ETIMEDOUT', 'ENOBUFS', 'ENOENT', 'EACCES', 'EINTR', 'ENOMEM', 'ENOEXEC'].includes(error?.code) ? error.code : 'unknown';
    const status = Number.isInteger(error?.status) && error.status >= 0 && error.status <= 255 ? error.status : 'unknown';
    const signal = ['SIGKILL', 'SIGTERM', 'SIGABRT', 'SIGSEGV', 'SIGBUS', 'SIGINT', 'SIGPIPE', 'SIGHUP', 'SIGQUIT', 'SIGILL', 'SIGFPE', 'SIGXCPU', 'SIGXFSZ', 'SIGSYS'].includes(error?.signal) ? error.signal : 'unknown';
    // No raw cause: Node's default error rendering would print its captured
    // stderr, command and request even when the outer message is bounded.
    throw new Error(`Release archive failed (stage=${stage}; elapsedMs=${Math.round(performance.now() - started)}; timeoutMs=${LOCAL_ARCHIVE_TIMEOUT_MS}; code=${code}; status=${status}; signal=${signal})`);
  }
}
const CONTRACT_PATH = 'scripts/desktop-core/release-contract.json';
export const RELEASE_TARGETS = Object.freeze({
  linux: { os: 'linux', arch: 'amd64', core: 'sing-box', sidecar: 'libcronet.so', suffix: '.tar.gz', cgo: '0' },
  win: { os: 'windows', arch: 'amd64', core: 'sing-box.exe', sidecar: 'libcronet.dll', suffix: '.zip', cgo: '0' },
  'mac-x64': { os: 'darwin', arch: 'amd64', core: 'sing-box', sidecar: null, suffix: '.tar.gz', cgo: '1' },
  'mac-arm64': { os: 'darwin', arch: 'arm64', core: 'sing-box', sidecar: null, suffix: '.tar.gz', cgo: '1' },
});
const hex = (v, n) => typeof v === 'string' && new RegExp(`^[a-f0-9]{${n}}$`).test(v);
const positive = (v) => Number.isSafeInteger(v) && v > 0;
const same = (a, b, message) => demand(canonical(a) === canonical(b), message);
// Match producer Python json.dumps(sort_keys=True, separators=(',', ':'), ensure_ascii=True).
export const portableCanonical = (v) => canonical(v).replace(/[\u007f-\uffff]/g,
  (c) => `\\u${c.charCodeAt(0).toString(16).padStart(4, '0')}`);
const json = (path) => JSON.parse(readFileSync(path, 'utf8'));

export function releasePlatforms(argv) {
  const flags = argv.filter((a) => a.startsWith('--platform='));
  demand(flags.length <= 1 && argv.filter((a) => a === '--force').length <= 1
    && argv.every((a) => a === '--force' || a.startsWith('--platform=')), 'Release mode accepts only --platform= and --force');
  const keys = flags.length ? flags[0].slice('--platform='.length).split(',') : Object.keys(RELEASE_TARGETS);
  demand(keys.length && new Set(keys).size === keys.length && keys.every((k) => Object.hasOwn(RELEASE_TARGETS, k)), 'Unknown/duplicate/empty App architecture selection');
  return keys;
}

export function regularPath(path, directory = false) {
  let at = resolve(path);
  demand(directory ? lstatSync(at).isDirectory() : lstatSync(at).isFile(), `Non-regular release path: ${at}`);
  for (at = dirname(at);;) {
    demand(lstatSync(at).isDirectory(), `Linked release ancestor: ${at}`);
    const parent = dirname(at); if (parent === at) break; at = parent;
  }
}
function checkedBytes(path, pin) {
  regularPath(path); const bytes = readFileSync(path);
  demand(bytes.length === pin.bytes && digest(bytes) === pin.sha256, `Release bytes differ: ${path}`);
  return bytes;
}
function validateRow(contract, key, row) {
  return validateUpstreamRow(contract, key, row, RELEASE_TARGETS);
}

export function validateReleaseContract(contract) {
  demand(isUpstreamRelease(contract), 'Unsupported desktop release contract');
  return validateUpstreamContract(contract, RELEASE_TARGETS);
}

// Shared with the existing Rust embedded dependency gate. V2 preserves all
// Go replacement rows; the sing-tun gate must select exactly its own module.
export function releaseSingTunDependency(row) {
  demand(Array.isArray(row.buildGraph?.localReplacements), 'Upstream replacement inventory missing');
  const selected = row.buildGraph.localReplacements.filter((item) => item.module === 'github.com/sagernet/sing-tun');
  demand(selected.length === 1 && selected[0].replacement?.module === './third_party/sing-tun'
    && selected[0].replacement.version === '(devel)' && selected[0].replacement.h1 === '', 'Unique upstream sing-tun replacement required');
  const dependency = selected[0];
  demand(dependency?.module === 'github.com/sagernet/sing-tun' && typeof dependency.version === 'string'
    && dependency.version.length > 0 && typeof dependency.h1 === 'string', 'Release sing-tun identity differs');
  return { module: dependency.module, version: dependency.version, h1: dependency.h1 };
}

// The committed selection is only the contract path and digest; every other
// identity is read from the digest-bound contract itself.
export function loadReleaseContract(root, manifest) {
  const selection = manifest.desktopRelease;
  demand(selection?.schema === 'polaris-app-release-selection-v1' && selection.contractPath === CONTRACT_PATH
    && hex(selection.contractSha256, 64) && Object.keys(selection).length === 3, 'Explicit release selection incomplete');
  const file = join(root, selection.contractPath); regularPath(file);
  const bytes = readFileSync(file); demand(bytes.length <= 8 * 1024 * 1024 && digest(bytes) === selection.contractSha256, 'Reviewed consumer contract SHA differs');
  return validateReleaseContract(JSON.parse(bytes));
}

// App installation identity for the upstream layout. Original producer rows stay intact.
export const UPSTREAM_INSTALLATION_DOMAIN = 'polaris-upstream-native-installation-v1\0';
export function releaseInstallationContract(root, manifest) {
  const contract = loadReleaseContract(root, manifest);
  const raw = readFileSync(join(root, manifest.desktopRelease.contractPath));
  demand(digest(raw) === manifest.desktopRelease.contractSha256, 'Installation contract changed during verification');
  const providerFingerprint = digest(Buffer.concat([Buffer.from(UPSTREAM_INSTALLATION_DOMAIN), raw]));
  return { ...contract, platforms: Object.fromEntries(Object.entries(contract.platforms).map(([key, row]) => {
    demand(!Object.hasOwn(row, 'providerFingerprint') && !Object.hasOwn(row, 'providerIdentity'),
      'Upstream installation cannot borrow an unverified producer identity');
    return [key, { ...row, providerFingerprint }];
  })) };
}

// The producer preserves codesign display stderr; it does not supply App signature state.
// Derive a bound state/CDHash, then use the existing strict native observer on Mac.
function releaseMacCodeSignature(row, key) {
  if (!key.startsWith('mac-')) return null;
  const { state, cdHash, trustValidated } = row.signatureObservation ?? {};
  demand(trustValidated === false, 'Upstream signature trust must not be asserted');
  const signature = { state, cdHash }; validateMacCodeSignature(signature, key); return signature;
}

export function releasePayloadMembers(contract, key) {
  const row = contract.platforms[key]; validateRow(contract, key, row);
  return Object.keys(upstreamPayloadPins(contract, row)).sort().map((name) => `core-release/${name}`);
}

// Redirects are permitted only for immutable asset bytes, whose expected hash is already pinned.
// Do not spend a native runner waiting for a shared anonymous IP's hourly reset.
// One short, evidenced recovery remains shared by the whole consumption.
const PRIMARY_QUOTA_WAIT_MS = 2 * 60 * 1000;
const TRANSPORT_WAIT_MS = 1000;
const TRANSPORT_CODES = new Set(['EAI_AGAIN', 'ECONNRESET', 'ETIMEDOUT', 'ECONNREFUSED', 'ENOTFOUND',
  'EACCES', 'EPERM', 'ABORT_ERR', 'ERR_TLS_CERT_ALTNAME_INVALID', 'CERT_HAS_EXPIRED',
  'DEPTH_ZERO_SELF_SIGNED_CERT', 'UNABLE_TO_VERIFY_LEAF_SIGNATURE',
  'UND_ERR_CONNECT_TIMEOUT', 'UND_ERR_HEADERS_TIMEOUT', 'UND_ERR_BODY_TIMEOUT',
  'UND_ERR_SOCKET', 'UND_ERR_ABORTED', 'UND_ERR_DESTROYED']);
const TRANSIENT_TRANSPORT_CODES = new Set(['EAI_AGAIN', 'ECONNRESET', 'ETIMEDOUT',
  'UND_ERR_CONNECT_TIMEOUT', 'UND_ERR_HEADERS_TIMEOUT', 'UND_ERR_SOCKET']);
function canonicalPublicAPI(endpoint, asset, repository = REPO) {
  kernelRepositoryIdentity(repository);
  return !asset && endpoint.protocol === 'https:' && endpoint.hostname === 'api.github.com'
    && !endpoint.port && !endpoint.username && !endpoint.password && !endpoint.hash && !endpoint.search
    && (endpoint.pathname === `/repos/${repository}` || endpoint.pathname.startsWith(`/repos/${repository}/`));
}
// Installation credentials are opaque Bearer values, not a frozen GitHub token-prefix alphabet.
// RFC 6750 b64token excludes whitespace/control bytes while permitting JWT dots and padding.
function releaseAPIToken(env) {
  if (env.GITHUB_ACTIONS !== 'true' || !env.POLARIS_RELEASE_API_TOKEN) return null;
  demand(typeof env.POLARIS_RELEASE_API_TOKEN === 'string'
    && env.POLARIS_RELEASE_API_TOKEN.length <= 4096
    && /^[A-Za-z0-9._~+/-]+=*$/.test(env.POLARIS_RELEASE_API_TOKEN), 'Invalid dedicated release API credential');
  return env.POLARIS_RELEASE_API_TOKEN;
}
function quotaDetail(response, token) {
  return ['x-github-request-id', 'x-ratelimit-limit', 'x-ratelimit-remaining',
    'x-ratelimit-reset', 'x-ratelimit-resource', 'retry-after'].map((name) => {
    const value = response.headers.get(name);
    const safe = value !== null && value !== token && (name === 'x-github-request-id'
      ? /^[A-Fa-f0-9:.-]{1,128}$/.test(value) : name === 'x-ratelimit-resource'
        ? ['core', 'search'].includes(value) : /^[0-9]{1,20}$/.test(value));
    return `${name}=${safe ? value : 'unknown'}`;
  }).join(', ');
}
const validETag = (value) => typeof value === 'string' && /^(?:W\/)?"[\x21\x23-\x7e]{1,256}"$/.test(value);
// If-None-Match uses weak comparison (RFC 9110 section 13.1.2).
// Header names are case-insensitive; opaque validator bytes are not.
const matchingETag = (left, right) => validETag(left) && validETag(right)
  && left.replace(/^W\//, '') === right.replace(/^W\//, '');
// Keep only a closed code vocabulary. Never copy error messages, addresses,
// URLs, signed queries or a raw cause into the public diagnostic. Mixed,
// unknown, cyclic or clipped cause graphs cannot authorize recovery.
function transportObservation(error) {
  const codes = new Set(), seen = new Set(); let complete = true, cancelled = false, kind = 'Error';
  const visit = (item, depth) => {
    if (!item || typeof item !== 'object' || depth > 4 || seen.size >= 16 || seen.has(item)) {
      complete = false; return;
    }
    seen.add(item);
    try {
      const name = item.name;
      if (depth === 0 && ['AbortError', 'TimeoutError', 'TypeError', 'Error', 'AggregateError'].includes(name)) kind = name;
      if (['AbortError', 'TimeoutError'].includes(name)) cancelled = true;
      const code = item.code, cause = item.cause, errors = item.errors;
      if (code !== undefined) {
        if (TRANSPORT_CODES.has(code)) codes.add(code);
        else complete = false;
      }
      const children = cause === undefined ? [] : [cause];
      if (errors !== undefined) {
        if (!Array.isArray(errors) || errors.length > 16) complete = false;
        else children.push(...errors);
      }
      if (!children.length && code === undefined) complete = false;
      for (const child of children) visit(child, depth + 1);
    } catch { complete = false; }
  };
  visit(error, 0);
  const sorted = [...codes].sort();
  return { kind, codes: sorted, complete,
    retryable: complete && !cancelled
      && sorted.length > 0 && sorted.every((code) => TRANSIENT_TRANSPORT_CODES.has(code)) };
}
const transportDetail = (observation) => `${observation.kind}; codes=${observation.codes.join(',') || 'unknown'}; causeComplete=${observation.complete}`;
// One evidenced primary-quota recovery across the entire consumption, including
// both public snapshots. Asset denials and unknown API denials stay fail-closed.
export function releasePublicReader(options = {}) {
  const quotaBudget = { available: true };
  const transportBudget = { available: true };
  const metadataCache = new Map(), observationBudget = { available: true }, conditionalObservationBudget = { available: true };
  const apiToken = releaseAPIToken(options.env ?? RELEASE_API_ENV);
  const now = options.now ?? Date.now;
  const supplied = options.deadlineMs ?? process.env.POLARIS_RELEASE_JOB_DEADLINE_MS;
  const deadlineMs = supplied === undefined ? now() + 120 * 60 * 1000 : Number(supplied);
  demand((supplied === undefined || /^[0-9]+$/.test(String(supplied)))
    && Number.isSafeInteger(deadlineMs) && deadlineMs > 0 && deadlineMs <= now() + 180 * 60 * 1000,
  'Invalid absolute release-consumer deadline');
  return (url, limit, asset = false) => publicBytes(url, limit, asset,
    { ...options, apiToken, quotaBudget, transportBudget, deadlineMs, metadataCache, observationBudget, conditionalObservationBudget });
}
function primaryQuotaDelay(response, endpoint, asset, now, authenticated, repository) {
  const reset = response.headers.get('x-ratelimit-reset'), retry = response.headers.get('retry-after');
  if (!canonicalPublicAPI(endpoint, asset, repository) || ![403, 429].includes(response.status)
    || !(authenticated ? ['1000', '15000'] : ['60']).includes(response.headers.get('x-ratelimit-limit'))
    || response.headers.get('x-ratelimit-remaining') !== '0'
    || response.headers.get('x-ratelimit-resource') !== 'core' || !/^[0-9]+$/.test(reset ?? '')
    || (retry !== null && !/^[0-9]+$/.test(retry))) return null;
  const resetMs = Number(reset) * 1000;
  if (!Number.isSafeInteger(resetMs) || resetMs <= now) return null;
  const delay = Math.max(resetMs - now + 1000, retry === null ? 0 : Number(retry) * 1000);
  return Number.isSafeInteger(delay) && delay > 0 ? delay : null;
}
export async function publicBytes(url, limit, asset = false, options = {}) {
  const repository = options.repository ?? REPO;
  kernelRepositoryIdentity(repository);
  const request = options.fetch ?? fetch;
  const token = options.apiToken ?? releaseAPIToken(options.env ?? RELEASE_API_ENV);
  for (let redirects = 0; redirects <= 3;) {
    const endpoint = new URL(url);
    const headers = { 'User-Agent': 'Polaris-release-consumer', Accept: 'application/vnd.github+json' };
    const metadata = canonicalPublicAPI(endpoint, asset, repository), authenticated = metadata && token !== null;
    if (authenticated) headers.Authorization = `Bearer ${token}`;
    const cached = metadata ? options.metadataCache?.get(url) : null;
    if (cached) headers['If-None-Match'] = cached.etag;
    // Assets/CDN redirects remain anonymous; API redirects fail closed. Never
    // put a credential, response body, signed query or raw error in diagnostics.
    const stage = asset ? (redirects ? 'asset-redirect' : 'asset') : 'public-api';
    const location = `stage=${stage} host=${endpoint.hostname} path=${endpoint.pathname}`;
    const now = (options.now ?? Date.now)();
    const remaining = options.deadlineMs === undefined ? 60000 : options.deadlineMs - now;
    demand(remaining > 0, `Release consumer job deadline exceeded (${location})`);
    let response;
    try {
      response = await request(url, { redirect: 'manual', signal: AbortSignal.timeout(Math.min(60000, remaining)), headers });
    } catch (error) {
      const observation = transportObservation(error);
      if (options.transportBudget?.available && observation.retryable && canonicalPublicAPI(endpoint, asset, repository)
        && (options.now ?? Date.now)() + TRANSPORT_WAIT_MS < options.deadlineMs) {
        options.transportBudget.available = false;
        (options.report ?? console.error)(`Release transport retry ${TRANSPORT_WAIT_MS}ms (${location}; ${transportDetail(observation)})`);
        await (options.sleep ?? ((ms) => new Promise((resolve) => setTimeout(resolve, ms))))(TRANSPORT_WAIT_MS);
        continue;
      }
      throw new Error(`Release request failed (${location}; ${transportDetail(observation)})`);
    }
    const cancelBody = async () => {
      try { await response.body?.cancel(); }
      catch (error) { throw new Error(`Release body cancellation failed (${location}; ${transportDetail(transportObservation(error))})`); }
    };
    if (response.status === 304) {
      await cancelBody();
      const etag = response.headers.get('etag');
      const intact = Boolean(cached && cached.bytes.length === cached.size && digest(cached.bytes) === cached.sha256);
      const withinLimit = Boolean(cached && cached.size <= limit);
      const validator = etag === null ? 'absent' : !validETag(etag) ? 'invalid'
        : !cached || !matchingETag(etag, cached.etag) ? 'different'
          : etag === cached.etag ? 'exact' : 'weak-equivalent';
      // Categorical diagnostics never expose validators, credentials or bodies.
      demand(metadata && cached && ['absent', 'exact', 'weak-equivalent'].includes(validator)
        && intact && withinLimit,
      `Release conditional response has no intact matching metadata (${location}; status=304; cached=${Boolean(cached)}; validator=${validator}; intact=${intact}; withinLimit=${withinLimit})`);
      if (validator === 'weak-equivalent' && options.conditionalObservationBudget?.available) {
        options.conditionalObservationBudget.available = false;
        (options.report ?? console.error)(`Release conditional observation (${location}; status=304; validator=weak-equivalent; intact=true; withinLimit=true)`);
      }
      return Buffer.from(cached.bytes);
    }
    if ([301, 302, 303, 307, 308].includes(response.status)) {
      await cancelBody(); const next = new URL(response.headers.get('location') ?? '', url);
      demand(asset && next.protocol === 'https:' && !next.username && !next.password && !next.port
        && ['release-assets.githubusercontent.com', 'objects.githubusercontent.com'].includes(next.hostname),
      'Unapproved release redirect');
      url = next.href; redirects++; continue;
    }
    if (!response.ok) {
      await cancelBody();
      const quota = `apiAuth=${authenticated ? 'actions' : 'anonymous'}; ${quotaDetail(response, token)}`;
      const delay = options.quotaBudget?.available
        ? primaryQuotaDelay(response, endpoint, asset, (options.now ?? Date.now)(), authenticated, repository) : null;
      if (delay !== null && delay <= PRIMARY_QUOTA_WAIT_MS
        && (options.now ?? Date.now)() + delay < options.deadlineMs) {
        options.quotaBudget.available = false;
        (options.report ?? console.error)(`Release primary quota wait ${delay}ms (${location}; ${quota})`);
        await (options.sleep ?? ((ms) => new Promise((resolve) => setTimeout(resolve, ms))))(delay);
        continue;
      }
      const budget = delay !== null && delay > PRIMARY_QUOTA_WAIT_MS ? '; primary-quota-wait-budget-exceeded' : '';
      throw new Error(`Release HTTP ${response.status} (${location}; ${quota}${budget})`);
    }
    const parts = []; let size = 0;
    let oversized = false;
    try {
      for await (const part of response.body) {
        size += part.length;
        if (size > limit) { oversized = true; break; }
        parts.push(part);
      }
    } catch (error) {
      throw new Error(`Release body read failed (${location}; ${transportDetail(transportObservation(error))})`);
    }
    demand(!oversized, 'Release response exceeds pinned size');
    const bytes = Buffer.concat(parts);
    // Cache at most five metadata endpoints under the existing 8 MiB limit. The
    // second snapshot still performs fresh HTTP validation; no stale/disk cache.
    const etag = response.headers.get('etag');
    if (metadata && options.metadataCache) {
      options.metadataCache.delete(url);
      if (validETag(etag) && bytes.length <= 8 * 1024 * 1024 && options.metadataCache.size < 5)
        options.metadataCache.set(url, { etag, bytes: Buffer.from(bytes), size: bytes.length, sha256: digest(bytes) });
    }
    if (metadata && options.observationBudget?.available) {
      options.observationBudget.available = false;
      (options.report ?? console.error)(`Release API observation (${location}; status=${response.status}; apiAuth=${authenticated ? 'actions' : 'anonymous'}; ${quotaDetail(response, token)})`);
    }
    return bytes;
  }
  throw new Error('Release redirect limit');
}
async function publicSnapshot(contract, read) {
  const request = async (path) => JSON.parse(await read(`https://api.github.com/repos/${contract.repository}${path}`, 8 * 1024 * 1024, false));
  const repo = await request(''); demand(repo.id === releaseRepositoryId(contract) && repo.full_name === contract.repository && repo.private === false, 'Repository identity differs');
  const release = await request(`/releases/${contract.releaseId}`);
  demand(release.id === contract.releaseId && release.tag_name === contract.tag && release.draft === false
    && release.html_url === `https://github.com/${contract.repository}/releases/tag/${contract.tag}`, 'Published release identity differs');
  const ref = await request(`/git/ref/tags/${contract.tag}`);
  demand(ref.ref === `refs/tags/${contract.tag}` && ref.object?.type === 'tag' && ref.object.sha === contract.tagObject, 'Annotated release tag differs');
  const tag = await request(`/git/tags/${contract.tagObject}`);
  demand(tag.sha === contract.tagObject && tag.tag === contract.tag && tag.object?.type === 'commit'
    && tag.object.sha === contract.sourceCommit, 'Release tag source differs');
  const commit = await request(`/git/commits/${contract.sourceCommit}`);
  demand(commit.sha === contract.sourceCommit && commit.tree?.sha === contract.sourceTree, 'Published source tree differs');
  demand(Array.isArray(release.assets) && new Set(release.assets.map((a) => a.id)).size === release.assets.length
    && new Set(release.assets.map((a) => a.name)).size === release.assets.length, 'Duplicate public assets');
  const assets = contractPublicAssets(contract).map((pin) => {
    const rows = release.assets.filter((a) => a.name === pin.name);
    demand(rows.length === 1, 'Pinned public asset missing'); const asset = rows[0];
    demand(asset.id === pin.assetId && asset.size === pin.bytes && asset.state === 'uploaded'
      && asset.browser_download_url === pin.url && (asset.digest == null || asset.digest === `sha256:${pin.sha256}`), 'Public asset identity differs');
    return { name: asset.name, id: asset.id, bytes: asset.size, url: asset.browser_download_url, digest: asset.digest ?? null };
  });
  return { repositoryId: repo.id, releaseId: release.id, tag: tag.sha, sourceCommit: commit.sha, sourceTree: commit.tree.sha, assets };
}
function coreHeader(bytes, target) {
  let ok = false;
  if (target.os === 'linux') ok = bytes.length >= 20 && bytes.subarray(0, 6).equals(Buffer.from([127, 69, 76, 70, 2, 1])) && bytes.readUInt16LE(18) === 62;
  else if (target.os === 'darwin') ok = bytes.length >= 32 && bytes.readUInt32LE(0) === 0xfeedfacf && bytes.readUInt32LE(4) === (target.arch === 'amd64' ? 0x1000007 : 0x100000c);
  else if (bytes.length >= 64 && bytes.toString('ascii', 0, 2) === 'MZ') {
    const offset = bytes.readUInt32LE(60);
    ok = offset + 6 <= bytes.length && bytes.toString('ascii', offset, offset + 4) === 'PE\0\0' && bytes.readUInt16LE(offset + 4) === 0x8664;
  }
  demand(ok, 'Actual release core architecture differs');
}
function actualBuildInfo(raw, goVersion) {
  const settings = {}, deps = [], replacements = [];
  demand(raw.split('\n')[0].endsWith(`: ${goVersion}`), 'Actual release Go version differs');
  demand(raw.includes('\tpath\tgithub.com/sagernet/sing-box/cmd/sing-box\n')
    && raw.includes('\tmod\tgithub.com/sagernet/sing-box\t'), 'Actual Go module identity differs');
  for (const line of raw.split('\n')) {
    const parts = line.split('\t');
    if (parts[1] === 'build') {
      const at = parts[2].indexOf('='); demand(at > 0 && !Object.hasOwn(settings, parts[2].slice(0, at)), 'Duplicate/invalid BuildInfo setting');
      let value = parts[2].slice(at + 1);
      if (value.startsWith('\"')) value = JSON.parse(value);
      demand(typeof value === 'string', 'Actual upstream BuildInfo setting differs');
      settings[parts[2].slice(0, at)] = value;
    } else if (parts[1] === 'dep') deps.push({ module: parts[2], version: parts[3], h1: parts[4] ?? '' });
    else if (parts[1] === '=>') {
      demand(deps.length > 0 && typeof parts[2] === 'string' && parts[2].length > 0 && typeof parts[3] === 'string', 'Actual upstream replacement differs');
      replacements.push({ ...deps.pop(), replacement: { module: parts[2], version: parts[3], h1: parts[4] ?? '' } });
    }
  }
  demand(replacements.length > 0 && deps.length && deps.every((d) => /^h1:[A-Za-z0-9+/]+=$/.test(d.h1))
    && new Set([...deps, ...replacements].map((d) => d.module)).size === deps.length + replacements.length, 'Actual module graph incomplete');
  const acquisition = deps.filter((d) => d.module.startsWith('github.com/sagernet/cronet-go/lib/'));
  return { settings, normalModules: deps.filter((d) => !acquisition.includes(d)), prebuiltAcquisitionModules: acquisition, localReplacements: replacements };
}
export function verifyReleaseCore(path, row, key, run = execFileSync, host = process.platform,
  signatureRun = run === execFileSync ? undefined : run, contract = undefined) {
  const bytes = checkedBytes(path, row.core); const target = RELEASE_TARGETS[key]; coreHeader(bytes, target);
  const options = { encoding: 'utf8', env: { ...process.env, GOTOOLCHAIN: 'local', GOWORK: 'off' },
    maxBuffer: 8 * 1024 * 1024, timeout: LOCAL_METADATA_TIMEOUT_MS, killSignal: 'SIGKILL' };
  const raw = String(run('go', ['version', '-m', path], options)).trimEnd() + '\n';
  if (contract) demand(row.sourceCommit === contract.sourceCommit && row.sourceTree === contract.sourceTree,
    'Actual core/contract source pair differs');
  const graph = actualBuildInfo(raw, row.goVersion);
  const normalized = (value) => { const lines = value.trimEnd().split(/\r?\n/);
    demand(lines[0].endsWith(row.goVersion), 'Original upstream BuildInfo Go version differs');
    return [row.goVersion, ...lines.slice(1)].join('\n'); };
  same(normalized(raw), normalized(row.rawBuildInfo), 'Actual upstream raw BuildInfo differs');
  same(graph, row.buildGraph, 'Actual release BuildInfo/module graph differs');
  const settings = graph.settings;
  demand(settings.GOOS === target.os && settings.GOARCH === target.arch && settings.CGO_ENABLED === target.cgo
    && (!Object.hasOwn(settings, 'vcs.revision') || settings['vcs.revision'] === row.sourceCommit)
    && (!Object.hasOwn(settings, 'vcs.modified') || settings['vcs.modified'] === 'false')
    && settings['-trimpath'] === 'true', 'Actual release build settings differ');
  same(settings['-tags']?.replaceAll('"', '').split(',').sort(), row.tags.split(',').sort(), 'Actual release feature tags differ');
  demand(String(run('go', ['tool', 'buildid', path], options)).trim() === row.buildID, 'Actual release BuildID differs');
  const signature = releaseMacCodeSignature(row, key);
  if (host === 'darwin' && signature) {
    same(observeMacCodeSignature(path, key, signatureRun), signature, 'Actual release Mac signature state/CDHash differs');
    checkedBytes(path, row.core); // Inspection must preserve the original downloaded bytes.
  }
  return graph;
}
function verifyExtracted(directory, rootName, row, key, run, host, signatureRun, contract) {
  const root = join(directory, rootName);
  const { core, sidecar } = row;
  verifyReleaseCore(join(root, core.member), row, key, run, host, signatureRun, contract);
  if (sidecar) inspectCronetBytes(checkedBytes(join(root, sidecar.member), sidecar), key);
  return root;
}
function directoryTree(path) {
  regularPath(path, true);
  for (const name of readdirSync(path)) {
    const child = join(path, name), st = lstatSync(child);
    if (st.isDirectory()) directoryTree(child); else regularPath(child);
  }
}
function directoryFingerprint(path) {
  directoryTree(path); const records = [];
  function walk(at, prefix = '') {
    records.push({ path: prefix, mode: lstatSync(at).mode & 0o777 });
    for (const name of readdirSync(at).sort()) {
      const child = join(at, name), relative = prefix + name, st = lstatSync(child);
      if (st.isDirectory()) walk(child, relative + '/');
      else records.push({ path: relative, mode: st.mode & 0o777, sha256: digest(readFileSync(child)) });
    }
  }
  walk(path); return digest(portableCanonical(records));
}
function saveJournal(lock, work, value) {
  const temporary = join(work, 'journal-next');
  const file = openSync(temporary, 'wx');
  try { writeFileSync(file, JSON.stringify(value) + '\n'); fsyncSync(file); } finally { closeSync(file); }
  renameSync(temporary, lock);
}
function processAlive(pid) {
  try { process.kill(pid, 0); return true; } catch (error) { return error.code !== 'ESRCH'; }
}
// Explicit recovery only. A live owner or changed resource tree is never removed.
export function recoverRelease(root, options = {}) {
  const resources = join(root, 'resources'), lock = join(resources, '.release-consumer.lock'); regularPath(lock);
  const journal = json(lock);
  demand(journal.schema === 'polaris-release-consumer-lock-v1' && positive(journal.pid)
    && !(options.isAlive ?? processAlive)(journal.pid), 'Release owner still active or recovery journal invalid');
  demand(typeof journal.work === 'string' && /^\.release-consumer-[A-Za-z0-9]+$/.test(journal.work), 'Recovery work path invalid');
  const work = join(resources, journal.work); regularPath(work, true);
  if (journal.phase !== 'acquiring') {
    demand(['replacing', 'committed'].includes(journal.phase) && Array.isArray(journal.directories)
      && journal.directories.length >= 2 && journal.directories.length <= 5
      && new Set(journal.directories.map((r) => r.key)).size === journal.directories.length
      && journal.directories.at(-1).key === '.release-receipts', 'Recovery directory inventory invalid');
    for (const row of journal.directories) {
      demand((Object.hasOwn(RELEASE_TARGETS, row.key) || row.key === '.release-receipts')
        && hex(row.staged, 64) && (row.original === null || hex(row.original, 64)), 'Recovery fingerprint invalid');
      const dest = join(resources, row.key), backup = join(work, `old-${row.key}`);
      if (journal.phase === 'committed') {
        demand(existsSync(dest) && directoryFingerprint(dest) === row.staged, 'Committed resources changed; recovery retained');
        if (existsSync(backup)) demand(row.original && directoryFingerprint(backup) === row.original, 'Original backup changed');
      } else if (existsSync(backup)) {
        demand(row.original && directoryFingerprint(backup) === row.original, 'Original backup changed');
        if (existsSync(dest)) demand(directoryFingerprint(dest) === row.staged, 'Partial resources changed; recovery retained');
      } else if (row.original) demand(existsSync(dest) && directoryFingerprint(dest) === row.original, 'Original resources unavailable');
      else if (existsSync(dest)) demand(directoryFingerprint(dest) === row.staged, 'New resources changed; recovery retained');
    }
    // Validate the entire recovery plan before restoring any directory.
    if (journal.phase === 'replacing') for (const row of [...journal.directories].reverse()) {
      const dest = join(resources, row.key), backup = join(work, `old-${row.key}`);
      if (existsSync(backup)) {
        if (existsSync(dest)) rmSync(dest, { recursive: true });
        (options.rename ?? renameSync)(backup, dest);
      } else if (row.original === null && existsSync(dest)) rmSync(dest, { recursive: true });
    }
  }
  rmSync(work, { recursive: true }); rmSync(lock);
  return { schema: 'polaris-release-consumer-recovery-v1', phase: journal.phase, restoredPreviousResources: journal.phase === 'replacing' };
}
function replaceDirectories(resources, work, keys, rename) {
  const completed = [];
  try {
    for (const key of [...keys, '.release-receipts']) {
      const dest = join(resources, key), backup = join(work, `old-${key}`), stage = join(work, 'stage', key);
      if (existsSync(dest)) { directoryTree(dest); rename(dest, backup); }
      completed.push({ dest, backup, installed: false });
      rename(stage, dest); completed.at(-1).installed = true;
    }
  } catch (error) {
    try {
      for (const { dest, backup, installed } of completed.reverse()) {
        if (installed) rmSync(dest, { recursive: true });
        if (existsSync(backup)) rename(backup, dest);
      }
    } catch (rollback) { throw new Error(`Release rollback failed; recovery retained at ${work}: ${rollback.message}`, { cause: error }); }
    throw error;
  }
}

// Raw transport only: no receipt, metadata, authority or validation result is cached.
// Saturation stops new writes instead of deleting a blob another acquisition uses.
const BLOB_CACHE_BYTES = 1024 * 1024 * 1024;
const BLOB_CACHE_MEMBER_BYTES = 512 * 1024 * 1024; // Existing archive pin ceiling.
const BLOB_CACHE_ENTRIES = 64;
function releaseBlobCache(root, options) {
  const report = (state) => { try { (options.report ?? console.error)(`Release blob cache ${state}`); } catch {} };
  const disabled = { read: () => null, publish: () => {} };
  // Injected transports are unit/private callers, unless they explicitly select
  // their own cache. Every normal production entry uses the ordinary reader.
  if (options.blobCacheDir === false || (options.read && options.blobCacheDir === undefined)) return disabled;
  if (typeof process.getuid !== 'function' || !constants.O_NOFOLLOW) { report('unavailable'); return disabled; }
  const uid = process.getuid(); let directory, owner;
  const privateFile = (st) => st.isFile() && st.uid === uid && !(st.mode & 0o077) && st.nlink === 1;
  const intactDirectory = () => {
    const st = lstatSync(directory);
    demand(st.isDirectory() && st.uid === uid && !(st.mode & 0o077)
      && st.dev === owner.dev && st.ino === owner.ino, 'Blob cache directory changed');
  };
  try {
    const parent = process.platform === 'darwin' ? join(homedir(), 'Library', 'Caches') : join(homedir(), '.cache');
    const requested = options.blobCacheDir ?? join(parent, 'polaris-release-blobs-v1');
    demand(typeof requested === 'string' && requested.length > 0, 'Invalid blob cache path');
    // Do not create or chmod an unknown parent; the normal system cache parent
    // must already exist. Canonicalize it before checking the complete ancestry.
    directory = join(realpathSync(dirname(resolve(requested))), basename(resolve(requested)));
    const source = realpathSync(root);
    demand(directory !== source && !directory.startsWith(source + sep), 'Blob cache is inside source');
    for (let at = dirname(directory);;) {
      const st = lstatSync(at);
      demand(st.isDirectory() && [0, uid].includes(st.uid)
        && (!(st.mode & 0o022) || (st.uid === 0 && (st.mode & 0o1000))), 'Unsafe blob cache ancestor');
      const parent = dirname(at); if (parent === at) break; at = parent;
    }
    try { mkdirSync(directory, { mode: 0o700 }); } catch (error) { if (error.code !== 'EEXIST') throw error; }
    owner = lstatSync(directory); intactDirectory();
    // Avoid placing a transport store on an SMB/network home or source share.
    // Unknown filesystems/platforms retain the ordinary download path.
    const localLinux = new Set([0xef53, 0x9123683e, 0x58465342, 0x2fc12fc1, 0x01021994, 0x794c7630]);
    demand(process.platform === 'linux' ? localLinux.has(statfsSync(directory).type)
      : process.platform === 'darwin' && owner.dev === lstatSync('/private/var/tmp').dev,
    'Blob cache filesystem is not a known local store');
    demand(Number.isSafeInteger(owner.blksize) && owner.blksize >= 512 && owner.blksize <= 1024 * 1024,
      'Blob cache allocation unit unavailable');
  } catch { report('unavailable'); return disabled; }
  report('ready');
  const name = (pin) => {
    demand(hex(pin.sha256, 64) && positive(pin.bytes) && pin.bytes <= BLOB_CACHE_MEMBER_BYTES, 'Invalid cached blob pin');
    return `${pin.sha256}.blob`;
  };
  const read = (pin, returnBytes = true) => {
    let fd;
    try {
      intactDirectory(); const path = join(directory, name(pin));
      fd = openSync(path, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
      const before = fstatSync(fd);
      demand(privateFile(before) && before.size === pin.bytes, 'Cached blob size/type differs');
      const bytes = Buffer.alloc(returnBytes ? pin.bytes : Math.min(256 * 1024, pin.bytes));
      const hash = createHash('sha256'); let offset = 0;
      while (offset < pin.bytes) {
        const at = returnBytes ? offset : 0;
        const count = readSync(fd, bytes, at, Math.min(256 * 1024, pin.bytes - offset), offset);
        demand(count > 0, 'Partial cached blob'); hash.update(bytes.subarray(at, at + count)); offset += count;
      }
      const after = fstatSync(fd);
      demand(privateFile(after) && after.size === before.size && after.ino === before.ino && after.dev === before.dev
        && after.mtimeMs === before.mtimeMs && after.ctimeMs === before.ctimeMs
        && hash.digest('hex') === pin.sha256, 'Cached blob bytes changed');
      intactDirectory(); return returnBytes ? bytes : true;
    } catch { return null; } finally { if (fd !== undefined) try { closeSync(fd); } catch {} }
  };
  // A short atomic directory lock covers every cache read/write. Busy or stale
  // ownership simply disables that operation; never steal another writer's lock.
  const locked = (action) => {
    const lock = join(directory, '.writer'); let acquired;
    try {
      intactDirectory(); mkdirSync(lock, { mode: 0o700 }); acquired = lstatSync(lock);
      intactDirectory(); return action();
    } catch { return null; } finally {
      if (acquired) try {
        intactDirectory(); const st = lstatSync(lock);
        if (st.isDirectory() && st.dev === acquired.dev && st.ino === acquired.ino) rmdirSync(lock);
      } catch {} // Cache I/O never replaces the normal consumer's result.
    }
  };
  const inventory = () => {
    const entries = []; let total = lstatSync(directory).blocks * 512 + lstatSync(join(directory, '.writer')).blocks * 512, count = 0;
    const stream = opendirSync(directory);
    try {
      for (let row; (row = stream.readSync()) !== null;) {
        demand(++count <= BLOB_CACHE_ENTRIES + 2, 'Blob cache entry bound');
        if (row.name === '.writer') continue;
        demand(/^[a-f0-9]{64}\.blob$/.test(row.name) || row.name === '.pending', 'Unknown blob cache entry');
        const st = lstatSync(join(directory, row.name));
        demand(privateFile(st) && st.size <= BLOB_CACHE_MEMBER_BYTES, 'Unsafe blob cache entry');
        demand(Number.isSafeInteger(st.blocks) && st.blocks >= 0, 'Blob cache allocation unavailable');
        total += Math.max(st.size, st.blocks * 512); entries.push({ name: row.name, bytes: st.size });
      }
    } finally { stream.closeSync(); }
    demand(entries.length <= BLOB_CACHE_ENTRIES && total <= BLOB_CACHE_BYTES, 'Blob cache byte/entry bound');
    return { entries, total };
  };
  return {
    read(pin) {
      const bytes = locked(() => { inventory(); return read(pin); });
      report(bytes ? 'hit' : 'miss');
      return bytes;
    },
    publish(blobs) {
      for (const { pin, file } of blobs) {
        const saved = locked(() => {
          const { entries, total } = inventory(), target = name(pin), existing = entries.find((row) => row.name === target);
          if (read(pin, false)) return true;
          if (entries.some((row) => row.name === '.pending') || (!existing && entries.length >= BLOB_CACHE_ENTRIES)
            || total + Math.ceil(pin.bytes / owner.blksize) * owner.blksize + 2 * owner.blksize > BLOB_CACHE_BYTES) return false;
          let input, output, owned;
          const temporary = join(directory, '.pending');
          try {
            input = openSync(file, constants.O_RDONLY | constants.O_NOFOLLOW | constants.O_NONBLOCK);
            demand(fstatSync(input).isFile() && fstatSync(input).size === pin.bytes, 'Verified transport file differs');
            output = openSync(temporary, constants.O_WRONLY | constants.O_CREAT | constants.O_EXCL | constants.O_NOFOLLOW, 0o600);
            owned = fstatSync(output); const buffer = Buffer.alloc(256 * 1024), hash = createHash('sha256'); let size = 0;
            for (;;) {
              const count = readSync(input, buffer, 0, buffer.length, null); if (!count) break;
              size += count; demand(size <= pin.bytes && size <= BLOB_CACHE_MEMBER_BYTES, 'Blob write bound');
              hash.update(buffer.subarray(0, count)); let offset = 0;
              while (offset < count) { const written = writeSync(output, buffer, offset, count - offset); demand(written > 0, 'Partial blob write'); offset += written; }
            }
            demand(size === pin.bytes && hash.digest('hex') === pin.sha256, 'Verified transport bytes changed');
            fsyncSync(output); closeSync(output); output = undefined; intactDirectory();
            renameSync(temporary, join(directory, target)); owned = undefined; return true;
          } finally {
            if (input !== undefined) closeSync(input); if (output !== undefined) closeSync(output);
            if (owned) try {
              intactDirectory(); const st = lstatSync(temporary);
              if (st.dev === owned.dev && st.ino === owned.ino) unlinkSync(temporary);
            } catch {}
          }
        });
        report(saved ? 'stored' : 'write-skipped');
      }
    },
  };
}

export async function consumeRelease(root, manifest, keys = Object.keys(RELEASE_TARGETS), options = {}) {
  demand(keys.length && new Set(keys).size === keys.length && keys.every((k) => Object.hasOwn(RELEASE_TARGETS, k)), 'Unknown/duplicate App architecture selection');
  const contract = loadReleaseContract(root, manifest), invoke = options.run ?? execFileSync, read = options.read ?? releasePublicReader({ repository: contract.repository });
  const withoutAPIToken = (execute) => (command, args, settings = {}) => {
    const env = { ...(settings.env ?? process.env) };
    delete env.POLARIS_RELEASE_API_TOKEN;
    return execute(command, args, { ...settings, env });
  };
  const run = withoutAPIToken(invoke);
  // codesign --display writes CDHash to stderr even on success. execFileSync
  // returns stdout only; preserve the observer's status/stdout/stderr contract.
  const signatureRun = invoke === execFileSync ? withoutAPIToken(spawnSync) : run;
  const blobCache = releaseBlobCache(root, options), cacheWrites = [];
  const resources = join(root, 'resources'); mkdirSync(resources, { recursive: true }); regularPath(resources, true);
  const lock = join(resources, '.release-consumer.lock');
  // Exclusivity is the continuously present wx-created path, not an open
  // descriptor: journal replacement changes its inode already. Windows refuses
  // replacing an open destination, so release this handle without unlinking.
  closeSync(openSync(lock, 'wx'));
  let work, preserve = false, replaced = false;
  try {
    work = mkdtempSync(join(resources, '.release-consumer-'));
    const journal = { schema: 'polaris-release-consumer-lock-v1', pid: process.pid, work: basename(work), phase: 'acquiring' };
    saveJournal(lock, work, journal);
    const before = await publicSnapshot(contract, read);
    const stage = join(work, 'stage'); mkdirSync(stage);
    const metadata = join(stage, '.release-receipts'); mkdirSync(metadata);
    const oldMetadata = join(resources, '.release-receipts');
    if (existsSync(oldMetadata)) { directoryTree(oldMetadata); cpSync(oldMetadata, metadata, { recursive: true }); }
    const receipts = {};
    for (const key of keys) {
      const row = contract.platforms[key], target = RELEASE_TARGETS[key];
      const archive = join(work, row.archive.name);
      const bytes = blobCache.read(row.releaseAsset) ?? await read(row.releaseAsset.url, row.releaseAsset.bytes, true);
      demand(bytes.length === row.releaseAsset.bytes && digest(bytes) === row.releaseAsset.sha256, 'Downloaded release archive SHA differs');
      writeFileSync(archive, bytes, { flag: 'wx' });
      cacheWrites.push({ pin: row.releaseAsset, file: archive });
      const extracted = join(work, `unpacked-${key}`), rootName = validateRow(contract, key, row);
      runColdArchive(run, 'native-archive-extraction', ['-B', join(dirname(fileURLToPath(import.meta.url)), 'release-archive.py'), archive, extracted],
        JSON.stringify({ archive: row.archive, root: rootName, target: row.target }));
      const source = verifyExtracted(extracted, rootName, row, key, run, options.host ?? process.platform, signatureRun, contract);
      const dest = join(stage, key); mkdirSync(dest);
      const existing = join(resources, key);
      if (existsSync(existing)) { directoryTree(existing); cpSync(existing, dest, { recursive: true }); }
      demand(!readdirSync(dest).some((name) => /^libcronet\.(so|dll|dylib)$/.test(name) && name !== target.sidecar), 'Existing resource contains a different Cronet sidecar');
      for (const name of [target.core, target.sidecar].filter(Boolean)) { copyFileSync(join(source, name), join(dest, name)); chmodSync(join(dest, name), lstatSync(join(source, name)).mode & 0o777); }
      const notices = join(dest, 'core-release'); rmSync(notices, { recursive: true, force: true }); mkdirSync(notices);
      for (const name of readdirSync(source).filter((n) => n !== target.core && n !== target.sidecar)) cpSync(join(source, name), join(notices, name), { recursive: true });
      for (const [name, content] of Object.entries(upstreamMetadata(contract, row))) {
        const path = join(notices, name); mkdirSync(dirname(path), { recursive: true });
        writeFileSync(path, content, { flag: 'wx', mode: 0o644 });
      }
      const facts = { platform: key, contractSha256: manifest.desktopRelease.contractSha256,
        releaseAsset: row.releaseAsset, producer: row,
        publicReadback: before,
        signaturePolicy: POLICY, macCodeSignature: releaseMacCodeSignature(row, key),
        actualAppAcceptance: 'pending native load, gvisor/mixed, feature and final carrier gates' };
      const receipt = { schema: 'polaris-app-release-consumption-v1', ...facts, fingerprint: digest(portableCanonical(facts)) };
      writeFileSync(join(metadata, `${key}.json`), JSON.stringify(receipt, null, 2) + '\n', { mode: 0o600 }); receipts[key] = receipt;
    }
    const after = await publicSnapshot(contract, read); same(after, before, 'Release identity changed during acquisition');
    writeFileSync(join(metadata, 'public-readback.json'), JSON.stringify({ schema: 'polaris-app-release-readback-v1', contractSha256: manifest.desktopRelease.contractSha256, before, after }, null, 2) + '\n', { mode: 0o600 });
    journal.phase = 'replacing'; journal.directories = [...keys, '.release-receipts'].map((key) => ({ key,
      original: existsSync(join(resources, key)) ? directoryFingerprint(join(resources, key)) : null,
      staged: directoryFingerprint(join(stage, key)) }));
    saveJournal(lock, work, journal);
    try {
      replaceDirectories(resources, work, keys, options.rename ?? renameSync); replaced = true;
      journal.phase = 'committed'; saveJournal(lock, work, journal);
    } catch (e) { if (replaced || e.message.startsWith('Release rollback failed;')) preserve = true; throw e; }
    blobCache.publish(cacheWrites);
    return receipts;
  } finally {
    if (!preserve) rmSync(lock);
    if (work && !preserve) rmSync(work, { recursive: true, force: true });
  }
}

export function verifyPackagedRelease(root, manifest, key, core, run = execFileSync, host = process.platform) {
  rejectLocalDistribution(root, dirname(core));
  demand(Object.hasOwn(RELEASE_TARGETS, key), 'Unknown App target');
  demand(!existsSync(join(root, 'resources/.release-consumer.lock')), 'Release acquisition/recovery still pending');
  const contract = loadReleaseContract(root, manifest), row = contract.platforms[key];
  const metadata = join(root, 'resources/.release-receipts'), file = join(metadata, `${key}.json`); regularPath(file);
  const receipt = json(file); const { fingerprint, schema, ...facts } = receipt;
  demand(schema === 'polaris-app-release-consumption-v1' && fingerprint === digest(portableCanonical(facts))
    && receipt.platform === key && receipt.contractSha256 === manifest.desktopRelease.contractSha256
    && receipt.signaturePolicy === POLICY, 'Release consumption receipt differs');
  same(receipt.producer, row, 'Saved original producer differs'); same(receipt.releaseAsset, row.releaseAsset, 'Saved asset identity differs');
  same(receipt.macCodeSignature, releaseMacCodeSignature(row, key), 'Saved Mac signature state/CDHash differs');
  regularPath(join(metadata, 'public-readback.json'));
  const readback = json(join(metadata, 'public-readback.json'));
  demand(readback.schema === 'polaris-app-release-readback-v1' && readback.contractSha256 === receipt.contractSha256, 'Saved public readback differs');
  same(readback.before, readback.after, 'Saved readback snapshots differ');
  same(readback.before, receipt.publicReadback, 'Consumption public snapshot differs');
  const snapshot = receipt.publicReadback;
  demand(snapshot?.repositoryId === releaseRepositoryId(contract) && snapshot.releaseId === contract.releaseId
    && snapshot.tag === contract.tagObject && snapshot.sourceCommit === contract.sourceCommit
    && snapshot.sourceTree === contract.sourceTree, 'Saved readback identity differs');
  same(snapshot.assets?.map(({ name, id, bytes, url }) => ({ name, id, bytes, url })),
    contractPublicAssets(contract)
      .map((p) => ({ name: p.name, id: p.assetId, bytes: p.bytes, url: p.url })), 'Saved public assets differ');
  // Both source and final carrier must retain the producer's bytes and permissions.
  const source = join(root, 'resources', key, row.core.member); checkedBytes(source, row.core);
  demand(((lstatSync(core).mode | lstatSync(source).mode) & 0o7000) === 0
      && (lstatSync(core).mode & 0o7777) === (lstatSync(source).mode & 0o7777), 'Final release core permissions differ');
  verifyReleaseCore(core, row, key, run, host, undefined, contract);
  if (row.sidecar) {
    const lib = join(dirname(core), row.sidecar.member), original = join(root, 'resources', key, row.sidecar.member);
    checkedBytes(original, row.sidecar); inspectCronetBytes(checkedBytes(lib, row.sidecar), key);
    demand(((lstatSync(lib).mode | lstatSync(original).mode) & 0o7000) === 0
      && (lstatSync(lib).mode & 0o7777) === (lstatSync(original).mode & 0o7777), 'Final release library permissions differ');
  } else demand(!readdirSync(dirname(core)).some((n) => /^libcronet\.(so|dll|dylib)$/.test(n)), 'Static carrier contains Cronet sidecar');
  for (const [relative, sha256] of Object.entries(upstreamPayloadPins(contract, row))) {
    if (relative === row.core.member || relative === row.sidecar?.member) continue;
    const original = join(dirname(source), 'core-release', relative), final = join(dirname(core), 'core-release', relative);
    regularPath(original); regularPath(final);
    demand(digest(readFileSync(original)) === sha256 && digest(readFileSync(final)) === sha256,
      'Release notice/provenance bytes differ in final carrier');
    demand(((lstatSync(original).mode | lstatSync(final).mode) & 0o7000) === 0
      && (lstatSync(original).mode & 0o7777) === (lstatSync(final).mode & 0o7777), 'Final release notice permissions differ');
  }
  return receipt;
}
