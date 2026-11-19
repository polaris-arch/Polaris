// Freeze and revalidate the same-run Apple handoff on the producer host.
import { createHash } from 'node:crypto';
import {
  closeSync, constants, copyFileSync, fstatSync, linkSync, lstatSync, mkdirSync, mkdtempSync,
  openSync, opendirSync, readlinkSync, readSync, realpathSync, rmSync, symlinkSync, writeFileSync,
} from 'node:fs';
import { homedir } from 'node:os';
import { dirname, isAbsolute, join, relative, resolve, sep } from 'node:path';
import { equal as same, filePin, localReplacementAliases, readReceipt, receiptDigest, receiptPolicy, treeInventory } from './component-admission.mjs';
import { load, require, ROOT, toolEnvironment, verifySource } from './fork-input.mjs';
import { appleResourceInputs, appleResources, verifyOriginal } from './mobile-consumption.mjs';
import { appleNativeEnvironment, createNativeRunner, privateDir, producerCode, toolFacts, writeJson } from './producer.mjs';

const MAX_FILES = 65_536, MAX_BYTES = 4 * 1024 ** 3;
const SCOPES = receiptPolicy.appleScopes;

export function requiredSlots(scope = process.env.POLARIS_APPLE_SCOPE ?? 'device-only') {
  require(typeof scope === 'string' && Object.hasOwn(SCOPES, scope), 'unknown Apple carrier scope');
  return [...SCOPES[scope]];
}

export function scopeFields(scope = process.env.POLARIS_APPLE_SCOPE ?? 'device-only') {
  return { carrierScope: scope, requiredSlots: requiredSlots(scope) };
}

function hostedRun(env) {
  const { GITHUB_RUN_ID: runId, GITHUB_RUN_ATTEMPT: attempt } = env;
  require(typeof runId === 'string' && /^[0-9]+$/.test(runId) && typeof attempt === 'string' && /^[0-9]+$/.test(attempt),
    'actual run/attempt required');
  return { runId, attempt };
}

export function location(env = process.env) {
  require(env.GITHUB_ACTIONS === 'true' && process.platform === 'darwin',
    'hosted Mac entry only; explicit local builder has separate admission');
  const { runId, attempt } = hostedRun(env);
  return privateDir(join(homedir(), 'Code/polaris-mobile-apple-ci', runId + '-' + attempt));
}

export async function context(runner = createNativeRunner()) {
  const candidate = process.env.POLARIS_APP_CANDIDATE;
  require(candidate && (await producerCode(runner.run)).candidate === candidate, 'handoff candidate differs');
  require(process.env.GITHUB_ACTIONS === 'true', 'same-run hosted handoff required');
  const repository = process.env.GITHUB_REPOSITORY;
  require(typeof repository === 'string' && repository.length, 'actual hosted repository required');
  return { candidate, ...hostedRun(process.env), repository };
}

function inside(base, path) {
  const written = relative(base, path);
  return !isAbsolute(written) && written !== '..' && !written.startsWith('..' + sep);
}

export function transportPin(path) {
  const options = { bigint: true };
  const before = lstatSync(path, options);
  require(before.isFile() && before.size > 0n && before.size <= BigInt(MAX_BYTES), 'bounded regular handoff transport required');
  const fd = openSync(path, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0));
  try {
    const identical = stat => stat.isFile() && stat.dev === before.dev && stat.ino === before.ino
      && stat.size === before.size && stat.mtimeNs === before.mtimeNs;
    require(identical(fstatSync(fd, options)), 'handoff transport changed before hashing');
    const hash = createHash('sha256'), block = Buffer.allocUnsafe(1024 ** 2);
    let bytes = 0;
    for (let n; (n = readSync(fd, block));) {
      bytes += n;
      require(bytes <= MAX_BYTES, 'handoff transport grew beyond budget');
      hash.update(block.subarray(0, n));
    }
    require(BigInt(bytes) === before.size && identical(fstatSync(fd, options)) && identical(lstatSync(path, options)),
      'handoff transport changed during hashing');
    return { bytes, sha256: hash.digest('hex') };
  } finally { closeSync(fd); }
}

function checkedRelative(name) {
  require(typeof name === 'string' && name.length && !/[\\\0]/.test(name) && Buffer.byteLength(name) <= 4096
    && name.split('/').every(part => part.length && part !== '.' && part !== '..' && Buffer.byteLength(part) <= 255),
  'unsafe handoff member');
  return name;
}

function checkoutMember(name) {
  return ['NOTICE', 'LICENSE', 'THIRD-PARTY-LICENSES.md'].includes(name)
    || ['resources/data/', 'resources/dashboard/', 'ui/dist/', 'src-tauri/gen/apple/assets/'].some(prefix => name.startsWith(prefix));
}

function selectedRow(path, base) {
  require(inside(base, path) && inside(base, realpathSync(path)), 'selected handoff byte escaped owned root');
  const info = lstatSync(path), mode = info.mode & 0o7777;
  if (info.isSymbolicLink()) {
    const link = readlinkSync(path);
    require(!isAbsolute(link), 'absolute framework link cannot be transported');
    return { link, mode };
  }
  if (info.isDirectory()) return { directory: true, mode };
  return { ...filePin(path, { allowEmpty: true }), mode };
}

function collectTree(paths, root) {
  paths.add(root);
  require(paths.size <= MAX_FILES, 'selected inventory file/directory budget');
  if (!lstatSync(root).isDirectory()) return;
  const directory = opendirSync(root);
  try {
    for (let entry; (entry = directory.readSync());) collectTree(paths, join(root, entry.name));
  } finally { directory.closeSync(); }
}

function selectedInventory(paths, base) {
  base = realpathSync(base);
  const complete = new Set(paths);
  for (const path of paths) {
    require(path !== base && inside(base, path), 'selected inventory escaped base');
    for (let parent = dirname(path); parent !== base; parent = dirname(parent)) {
      complete.add(parent);
      require(complete.size <= MAX_FILES, 'selected inventory file/directory budget');
    }
  }
  require(complete.size <= MAX_FILES, 'selected inventory file/directory budget');
  return Object.fromEntries([...complete].sort().map(path => [relative(base, path), selectedRow(path, base)]));
}

function replacementRoots(receipt, component) {
  const graph = receipt.input.moduleGraph, main = graph.filter(row => row.Main === true);
  require(main.length === 1 && typeof main[0].Dir === 'string', 'one actual main source view required');
  const source = main[0].Dir;
  require(isAbsolute(source) && inside(component, source) && realpathSync(source) === source && lstatSync(source).isDirectory(),
    'actual source view redirected/missing');
  const roots = [];
  for (const { Path: module, Replace: replacement } of graph) {
    if (!replacement || replacement.Version) continue;
    localReplacementAliases(replacement, source, replacement.Path, module);
    const directory = replacement.Dir;
    require(realpathSync(directory) === directory && lstatSync(directory).isDirectory(), 'local replacement source redirected/missing');
    roots.push({ module, path: replacement.Path, sourceRoot: source, directory });
    require(roots.length <= 1024, 'local replacement source count budget');
  }
  return roots;
}

function replacementSnapshots(receipt, component) {
  return replacementRoots(receipt, component).map(row => {
    const inventory = treeInventory(row.directory);
    require(inventory.some(file => file.type === 'file'), 'local replacement source bytes required');
    return { ...row, inventory, goMod: filePin(join(row.sourceRoot, 'go.mod')), goSum: filePin(join(row.sourceRoot, 'go.sum')) };
  });
}

function verifyReplacements(snapshots, receipt, component) {
  const expected = replacementRoots(receipt, component), sourceContract = receipt.input.sourceContract;
  require(Array.isArray(snapshots) && snapshots.length === expected.length, 'actual local replacement sources omitted');
  for (const [index, row] of expected.entries()) {
    const snapshot = snapshots[index];
    require(Object.entries(row).every(([key, value]) => snapshot[key] === value), 'local replacement source identity changed');
    require(same(filePin(join(row.sourceRoot, 'go.mod')), snapshot.goMod)
      && same(filePin(join(row.sourceRoot, 'go.sum')), snapshot.goSum)
      && same(treeInventory(row.directory), snapshot.inventory), 'actual local replacement source bytes/modes changed');
    require(snapshot.goMod.sha256 === sourceContract.goModSha256 && snapshot.goSum.sha256 === sourceContract.goSumSha256,
      'retained source identity bytes differ');
  }
}

// A NUL-delimited list and no recursion keep the native ZIP writer confined to
// the selected names. Keep the temporary archive on the destination filesystem;
// a final hardlink atomically claims the output without replacing earlier bytes.
export async function writeArchiveZip(root, output, entries, runner = createNativeRunner(), { maxBytes = MAX_BYTES } = {}) {
  require(Array.isArray(entries) && entries.length > 0 && entries.length <= MAX_FILES + 1
    && new Set(entries).size === entries.length && Number.isSafeInteger(maxBytes) && maxBytes >= 1024,
  'bounded distinct ZIP inventory required');
  for (const entry of entries) checkedRelative(entry);
  const folder = mkdtempSync(join(dirname(output), '.selected-zip-'));
  try {
    const list = join(folder, 'members'), staged = join(folder, 'owned.zip');
    writeFileSync(list, entries.join('\0') + '\0', { flag: 'wx', mode: 0o600 });
    // Darwin tar pads ZIP written to a pipe. Direct output preserves its exact
    // end record, while Darwin Bash's 1024-byte RLIMIT_FSIZE units cap the write.
    const result = await runner.runBounded(['/bin/bash', '-c', 'ulimit -f "$1" || exit; shift; exec /usr/bin/tar "$@"',
      'bounded-apple-zip', String(Math.floor(maxBytes / 1024)), '-c', '--format', 'zip', '--no-recursion',
      '-f', staged, '--null', '-T', list], {
      cwd: root, env: { ...process.env, COPYFILE_DISABLE: '1' }, stdoutLimit: 16 * 1024, stderrLimit: 256 * 1024,
    });
    require(result.complete, 'native bounded ZIP production did not complete');
    const info = lstatSync(staged);
    require(info.isFile() && info.size > 0 && info.size <= maxBytes, 'native bounded ZIP production did not complete');
    linkSync(staged, output);
  } finally { rmSync(folder, { recursive: true, force: true }); }
}

async function validateManifest(manifest, root, checkout, runner) {
  const scope = scopeFields(), current = await context(runner);
  require(manifest.schema === 'polaris-apple-component-handoff-v2' && manifest.carrierScope === scope.carrierScope
    && same(manifest.requiredSlots, scope.requiredSlots), 'Apple handoff declared scope/requiredSlots differs');
  const { fingerprint, ...facts } = manifest;
  require(fingerprint === receiptDigest(facts), 'Apple handoff manifest changed');
  require(same(manifest.context, current) && manifest.producerJob === 'component', 'handoff is not this run/attempt/component job');
  require(manifest.root === realpathSync(root) && manifest.checkout === realpathSync(checkout),
    'host canonical layout differs; original absolute paths cannot be relabelled');
  require(same(manifest.App, await producerCode(runner.run)) && same(manifest.sourceContract, load()), 'handoff App/source contract differs');
  require(Object.keys(manifest.files).length === 2 && Object.hasOwn(manifest.files, 'root') && Object.hasOwn(manifest.files, 'checkout'),
    'handoff root inventory differs');
  let total = 0, count = 0;
  for (const [namespace, rows] of Object.entries(manifest.files)) {
    require(rows && typeof rows === 'object' && !Array.isArray(rows), 'handoff inventory required');
    const base = realpathSync(namespace === 'root' ? root : checkout);
    const names = Object.keys(rows), checkoutParents = new Set();
    if (namespace === 'checkout') {
      for (const name of names.filter(checkoutMember)) {
        for (let parent = dirname(name); parent !== '.'; parent = dirname(parent)) checkoutParents.add(parent);
      }
    }
    for (const name of names) {
      checkedRelative(name);
      const wanted = rows[name];
      if (namespace === 'checkout') require(checkoutMember(name) || wanted.directory === true && checkoutParents.has(name),
        'unexpected checkout handoff member');
      total += wanted.bytes ?? 0;
      require(++count <= MAX_FILES && total <= MAX_BYTES, 'handoff selected bytes budget');
      require(same(selectedRow(join(base, name), base), wanted), 'imported handoff bytes/mode/link changed');
    }
  }
  for (const snapshot of manifest.localReplacementSources ?? []) {
    require(same(treeInventory(snapshot.directory), snapshot.inventory), 'retained local replacement inventory changed');
  }
  return manifest;
}

export async function freeze(root, runner = createNativeRunner()) {
  root = realpathSync(root);
  const tools = readReceipt(join(root, 'tools.json')), scope = scopeFields();
  require(tools.carrierScope === scope.carrierScope && same(tools.requiredSlots, scope.requiredSlots),
    'prepared Apple carrier scope differs before freeze');
  const component = tools.paths['component-work'], receiptPath = join(component, 'component-receipt.json');
  const receipt = readReceipt(receiptPath), sourceContract = load(), App = await producerCode(runner.run);
  require(component === join(root, 'component') && same(receipt.input.sourceContract, sourceContract)
    && same(receipt.input.producer, App), 'original component producer differs');
  const work = privateDir(join(root, 'handoff-work'));
  await runner.withLease(work, tools.paths.cache, sourceContract, async () => {
    const library = join(component, 'Libbox.xcframework');
    await verifyOriginal({ component: library, receiptPath, platform: 'apple', scratch: privateDir(join(work, 'check')), run: runner.run });
    const localReplacementSources = replacementSnapshots(receipt, component);
    const originals = new Set([join(root, 'tools.json'), tools.paths.gomobile, tools.paths.gobind,
      receiptPath, join(component, 'original-bind.log'), ...Object.keys(receipt.nativeInputs.files)]);
    for (const directory of [library, receipt.nativeInputs.generatorOutputRoot, ...localReplacementSources.map(row => row.directory)]) {
      collectTree(originals, directory);
    }
    for (const source of new Set(localReplacementSources.map(row => row.sourceRoot))) {
      originals.add(join(source, 'go.mod'));
      originals.add(join(source, 'go.sum'));
      require(same(await verifySource(source, sourceContract, { run: runner.run, generatorOutputs: true }),
        receipt.input.sourceObservation), 'replacement source view differs from original producer');
    }
    const checkoutPaths = new Set(Object.values(appleResourceInputs()).map(row => join(ROOT, row.source)));
    for (const name of ['ui/dist', 'resources/dashboard', 'src-tauri/gen/apple/assets']) {
      const directory = join(ROOT, name);
      require(lstatSync(directory).isDirectory(), 'actual generated frontend/dashboard/assets required');
      collectTree(checkoutPaths, directory);
    }
    const files = { root: selectedInventory(originals, root), checkout: selectedInventory(checkoutPaths, ROOT) };
    const rows = Object.values(files).flatMap(Object.values);
    require(rows.length <= MAX_FILES && rows.reduce((total, row) => total + (row.bytes ?? 0), 0) <= MAX_BYTES,
      'selected Apple handoff too large');
    const current = await context(runner);
    const manifest = { schema: 'polaris-apple-component-handoff-v2', ...scope, context: current,
      producerJob: process.env.GITHUB_JOB, root, checkout: ROOT, App, sourceContract, files,
      componentReceipt: filePin(receiptPath), componentFingerprint: receipt.fingerprint, localReplacementSources,
      tools: tools.tools, resources: appleResources(), devicePassed: false,
      signatureTrustValidated: false, publicationEligible: false };
    manifest.fingerprint = receiptDigest(manifest);
    await validateManifest(manifest, root, ROOT, runner);
    const manifestPath = join(root, 'component-handoff.json'), archive = join(root, 'component-handoff.zip');
    writeJson(manifestPath, manifest);
    const view = mkdtempSync(join(work, 'view-'));
    try {
      symlinkSync(root, join(view, 'root'), 'dir');
      symlinkSync(ROOT, join(view, 'checkout'), 'dir');
      linkSync(manifestPath, join(view, 'component-handoff.json'));
      const names = Object.entries(files).flatMap(([namespace, entries]) => Object.keys(entries).map(name => namespace + '/' + name));
      await writeArchiveZip(view, archive, [...names, 'component-handoff.json'], runner);
    } finally { rmSync(view, { recursive: true, force: true }); }
    await validateManifest(manifest, root, ROOT, runner);
    const transport = { schema: 'polaris-apple-handoff-transport-v1', context: await context(runner),
      archive: transportPin(archive), manifest: filePin(manifestPath) };
    transport.fingerprint = receiptDigest(transport);
    writeJson(join(root, 'component-transport.json'), transport);
  });
}

export async function verifyImport(component, cache, source, go, gomobile, gobind, developer, runner = createNativeRunner()) {
  const root = process.env.POLARIS_FORMAL_APPLE_ROOT || dirname(component);
  const facts = await validateManifest(readReceipt(join(root, 'component-handoff.json')), root, ROOT, runner);
  const { paths } = readReceipt(join(root, 'tools.json'));
  const actual = { 'component-work': component, cache, source, go, gomobile, gobind, developer };
  require(Object.entries(actual).every(([name, path]) => paths[name] === path), 'imported producer paths changed');
  for (const [name, path] of Object.entries({ go, gomobile, gobind })) {
    require(same(filePin(path), facts.tools[name]), 'actual imported tool bytes differ');
  }
  const value = load(), receiptPath = join(component, 'component-receipt.json'), receipt = readReceipt(receiptPath);
  const binding = receipt.input;
  require(same(filePin(receiptPath), facts.componentReceipt) && same(binding.producer, facts.App)
    && same(binding.sourceContract, value) && same(binding.tools.executables, facts.tools), 'original producer receipt differs');
  const { identities } = await toolFacts({ platform: 'apple', go, gomobile, gobind }, value,
    { ...process.env, DEVELOPER_DIR: developer }, runner.run);
  require(same(identities, binding.tools), 'actual consumer tool/SDK bytes differ');
  const scratch = privateDir(join(root, 'import-check'));
  if (Object.hasOwn(binding, 'nativeEnvironment')) {
    const nativeEnv = toolEnvironment(go, gomobile, gobind, cache, scratch, { developer });
    require(same(await appleNativeEnvironment(runner.run, go, nativeEnv), binding.nativeEnvironment),
      'actual consumer Go/native environment differs');
  }
  verifyReplacements(facts.localReplacementSources, receipt, component);
  await verifyOriginal({ component: join(component, 'Libbox.xcframework'), receiptPath, platform: 'apple', scratch, run: runner.run });
  return receipt;
}

export async function carrierSummary(root, slot, runner = createNativeRunner()) {
  require(requiredSlots().includes(slot), 'unrequested Apple carrier slot');
  const receiptPath = join(root, slot, 'app-carrier.json'), receipt = readReceipt(receiptPath), current = await context(runner);
  require(receipt.App.candidate === current.candidate && receipt.devicePassed === false, 'carrier candidate/device scope differs');
  const row = { schema: 'polaris-apple-carrier-handoff-v2', ...scopeFields(), context: current, slot,
    componentTransport: readReceipt(join(root, 'component-transport.json')), receipt, receiptPin: filePin(receiptPath),
    devicePassed: false, signatureTrustValidated: false, publicationEligible: false };
  row.fingerprint = receiptDigest(row);
  const { receipt: rustPath, bytes, sha256 } = receipt.rustInput;
  require(isAbsolute(rustPath) && inside(join(root, slot), rustPath) && same(filePin(rustPath), { bytes, sha256 }),
    'original Rust receipt changed');
  const summary = join(privateDir(join(root, 'summaries')), slot);
  mkdirSync(summary, { mode: 0o700 });
  writeJson(join(summary, 'carrier-' + slot + '.json'), row);
  for (const [path, name] of [[receiptPath, 'summary-app-carrier.json'], [rustPath, 'summary-rust-receipt.json']]) {
    const expected = filePin(path), output = join(summary, name);
    copyFileSync(path, output, constants.COPYFILE_EXCL);
    require(same(filePin(path), expected) && same(filePin(output), expected), 'original summary bytes changed during export');
  }
}
