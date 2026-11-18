#!/usr/bin/env node
// The command chooses a producer operation, never a runtime trust lane.
import { execFileSync } from 'node:child_process';
import { readFileSync, existsSync, lstatSync, openSync, fstatSync, readSync, closeSync, constants, realpathSync } from 'node:fs';
import { resolve, dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { selectedCoreManifest, verifySelectedCoreManifest, actualCoreFile, writeSelectedCoreManifest } from '../lib/core-identity.mjs';
import { portableCanonical } from './release-consumer.mjs';
import { digest, requireGraph as demand } from './source-graph.mjs';
export const LOCAL_MARKER = Buffer.from('POLARIS_SELECTION_LOCAL_DEVELOPMENT_V1');
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
const compiledSelection = 'crates/core-supervisor/src/selected_core/development_config.rs';
const selectionSchema = 'polaris-local-development-selection-v1';
const contractSchema = 'polaris-local-development-core-contract-v1';
export const localDirectory = (base) => join(base, 'resources/.local-development/linux');
const selectionExecutable = (path) => /^(?:polaris|Polaris|polaris-helper)(?:\.exe)?$/.test(path.replaceAll('\\', '/').split('/').at(-1));
export function rejectLocalExecutables(paths) {
  const selected = paths.filter(selectionExecutable);
  demand(selected.length <= 256, 'Distribution executable count exceeds budget');
  for (const input of selected) {
    // Resolve legitimate AppDir links, then retain/read the actual opened object.
    const path = realpathSync(input), before = lstatSync(path);
    demand(before.isFile() && !before.isSymbolicLink() && before.size <= 512 * 1024 ** 2, 'Unsafe distribution executable');
    const fd = openSync(path, constants.O_RDONLY | (constants.O_NOFOLLOW ?? 0) | (constants.O_NONBLOCK ?? 0));
    try {
      const opened = fstatSync(fd); demand(opened.dev === before.dev && opened.ino === before.ino, 'Distribution executable changed before scan');
      const block = Buffer.alloc(64 * 1024); let tail = Buffer.alloc(0), total = 0;
      for (let n; (n = readSync(fd, block, 0, block.length, null)) > 0;) {
        total += n; demand(total <= before.size, 'Distribution executable grew during scan');
        const data = Buffer.concat([tail, block.subarray(0, n)]);
        demand(!data.includes(LOCAL_MARKER), 'Formal package/publisher rejects a LocalDevelopmentSelection executable: ' + input);
        tail = data.subarray(Math.max(0, data.length - LOCAL_MARKER.length + 1));
      }
      const after = fstatSync(fd);
      demand(total === before.size && after.size === before.size && after.mtimeMs === before.mtimeMs && after.ctimeMs === before.ctimeMs, 'Distribution executable changed during scan');
    } finally { closeSync(fd); }
  }
}
export function rejectLocalDistribution(base, directory) {
  demand(!existsSync(join(base, 'resources/.local-development')), 'Formal distribution rejects local development compiler inputs');
  if (directory) rejectLocalExecutables(['polaris-helper', 'polaris', 'Polaris', 'polaris-helper.exe', 'polaris.exe', 'Polaris.exe'].map(name => join(directory,name)).filter(existsSync));
}

export function compilerSelectionSource(template, selectionBytes) {
  const { context, row, contractSha256 } = JSON.parse(selectionBytes);
  const pins = {
    source_commit: context.coreSource.commit,
    source_tree: context.coreSource.tree,
    producer_commit: context.producerSource.commit,
    producer_tree: context.producerSource.tree,
    target: row.target,
    go_version: row.goVersion,
    tags: row.tags,
    core_bytes: row.core.bytes,
    core_sha256: row.core.sha256,
    sidecar_bytes: row.sidecar.bytes,
    sidecar_sha256: row.sidecar.sha256,
    selection_sha256: digest(selectionBytes),
    contract_sha256: contractSha256,
  };
  const fields = Object.entries(pins).map(([name, value]) => name + ':' + portableCanonical(value)).join(',');
  return template.replace(/\r\n?/g, '\n').split('pub const COMPILED:', 1)[0] +
    'pub const COMPILED:Option<DevelopmentSelection>=Some(DevelopmentSelection{' + fields + '});\n';
}

// The pinned producer's quantities (member sizes, seals and process results)
// are exact integers. Keep the sorted two-space ASCII encoding of v1 receipts.
const receiptJson = (value) => portableCanonical(value, 2) + '\n';
const closed = (value, keys) => value && typeof value === 'object' && !Array.isArray(value) &&
  Object.keys(value).length === keys.length && keys.every((key) => Object.hasOwn(value, key));

export function localReceipt(template, { recipe, recipeSha256, draft, selection, contract }) {
  const verifying = selection !== undefined;
  let document;
  try {
    document = JSON.parse(verifying ? selection : draft, (_, value, context) => {
      if (typeof value === 'number') {
        demand(Number.isSafeInteger(value) && /^(0|-?[1-9][0-9]*)$/.test(context.source),
          'local receipt quantity is not an exact integer');
      }
      return value;
    });
  } catch (error) {
    if (!(error instanceof SyntaxError)) throw error;
    throw new Error('invalid local receipt JSON');
  }
  if (verifying) demand(selection === receiptJson(document), 'local selection is not the exact producer encoding');
  const fields = ['schema', 'configurationSha256', 'context', 'row', 'proof', 'policy'];
  if (verifying) fields.push('providerFingerprint', 'contractSha256');
  demand(closed(document, fields) && document.schema === selectionSchema, 'local selection shape differs');
  const { configurationSha256, context, row, proof, policy } = document;
  demand(configurationSha256 === recipeSha256, 'local compiler configuration changed');
  demand(closed(row, ['version', 'target', 'sourceCommit', 'sourceTree', 'goVersion', 'tags', 'variant',
    'buildGraph', 'rawBuildInfo', 'archive', 'core', 'sidecar', 'finalPackageCronetSmoke']), 'local producer row shape differs');
  demand(row.version === recipe.version && row.goVersion === recipe.goVersion && row.target === recipe.target &&
    row.tags === recipe.tags.join(','), 'local build declaration differs');
  demand(closed(context, ['schema', 'coreSource', 'producerSource', 'host']) &&
    context.schema === 'polaris-local-development-execution-context-v1' &&
    portableCanonical(context.coreSource) === portableCanonical(recipe.coreSource) &&
    portableCanonical(context.producerSource) === portableCanonical(recipe.producerSource) &&
    portableCanonical(context.host) === '{"arch":"amd64","os":"linux"}' &&
    row.sourceCommit === recipe.coreSource.commit && row.sourceTree === recipe.coreSource.tree,
  'local source context differs');
  demand(closed(row.sidecar, ['member', 'bytes', 'sha256']) && row.sidecar.member === 'libcronet.so' &&
    row.sidecar.bytes === recipe.sidecar.bytes && row.sidecar.sha256 === recipe.sidecar.sha256,
  'local sidecar is not the compiled trusted input');

  const providerFingerprint = digest(receiptJson({ configurationSha256, context, row, proof }));
  const { version, sourceCommit, sourceTree, archive, core, sidecar } = row;
  const contractBytes = receiptJson({ schema: contractSchema, version, sourceCommit, sourceTree,
    platforms: { linux: { core, sidecar, archive, sourceCommit, sourceTree, providerFingerprint } } });
  const contractSha256 = digest(contractBytes);
  if (verifying) {
    demand(document.providerFingerprint === providerFingerprint, 'local protected facts/provider differs');
    demand(contract === contractBytes && document.contractSha256 === contractSha256, 'local compiled contract differs');
    demand(template.replace(/\r\n?/g, '\n') === compilerSelectionSource(template, selection),
      'generated facts differ from the immutable Rust compiler configuration');
    return null;
  }
  const selectionBytes = receiptJson({ schema: selectionSchema, configurationSha256, context, row, proof, policy,
    providerFingerprint, contractSha256 });
  return { selection: selectionBytes, contract: contractBytes, compiled: compilerSelectionSource(template, selectionBytes),
    providerFingerprint };
}

export function verifyLocalSelection(base = root, installation = false) {
  const directory = localDirectory(base);
  // Fixed code and fixed input slot. No env selector or receipt chooses Cargo's lane.
  execFileSync('python3', [join(base, 'scripts/desktop-core/local-development.py'), 'verify'], { cwd: base, env: { ...process.env, GOENV: 'off', GOTOOLCHAIN: 'local', GOWORK: 'off', GOPROXY: 'off', GOSUMDB: 'off', PYTHONDONTWRITEBYTECODE: '1' }, stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: 2 * 1024 ** 2 });
  const contract = JSON.parse(readFileSync(join(directory, 'contract.json')));
  demand(contract.schema === contractSchema && Object.keys(contract.platforms).join() === 'linux', 'Local contract is Linux-only');
  if (installation) {
    const appVersion = JSON.parse(readFileSync(join(base, 'src-tauri/tauri.conf.json'))).version;
    const expected = selectedCoreManifest(contract, 'linux', appVersion);
    const saved = JSON.parse(readFileSync(join(directory, 'core-manifest.json')));
    demand(saved.id === expected.id && saved.appVersion === appVersion && saved.core?.platform === 'linux', 'Local installation selected content differs');
    demand(saved.schema === expected.schema, 'Local installation schema differs');
    for (const name of ['version', 'providerFingerprint', 'files']) demand(JSON.stringify(saved.core[name]) === JSON.stringify(expected.core[name]), 'Local installation identity differs: ' + name);
    const helper = join(directory, 'polaris-helper'); const bytes = readFileSync(helper);
    demand(bytes.includes(LOCAL_MARKER), 'Local installation requires a compiled LocalDevelopmentSelection Helper');
    actualCoreFile(helper, { bytes: bytes.length, sha256: saved.core.helperSha256 });
    verifySelectedCoreManifest(directory, directory, 'linux');
  }
  return contract;
}
export function prepareLocalInstallation(base, directory, mode) {
  demand(resolve(directory) === resolve(localDirectory(base)), 'Fixed local development input directory required');
  demand(['prepare', 'verify'].includes(mode), 'Local development has no maintenance-anchor or packaging mode');
  const contract = verifyLocalSelection(base, false);
  if (mode === 'verify') { verifyLocalSelection(base, true); return contract; }
  demand(mode === 'prepare', 'Local development has no maintenance-anchor or packaging mode');
  const helper = readFileSync(join(directory, 'polaris-helper'));
  demand(helper.includes(LOCAL_MARKER), 'Build the Helper with the explicit local development Cargo feature first');
  const appVersion = JSON.parse(readFileSync(join(base, 'src-tauri/tauri.conf.json'))).version;
  writeSelectedCoreManifest(directory, contract, 'linux', appVersion);
  verifyLocalSelection(base, true);
  return contract;
}
if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  const [mode, ...args] = process.argv.slice(2);
  if (mode === 'collect') {
    execFileSync('python3', [join(root, 'scripts/desktop-core/local-development.py'), 'collect', ...args], { cwd: root, stdio: 'inherit' });
  } else if (mode === 'receipt' && args.length === 0) {
    const material = localReceipt(readFileSync(join(root, compiledSelection), 'utf8'), JSON.parse(readFileSync(0)));
    process.stdout.write(JSON.stringify(material));
  } else {
    demand(args.length === 0 && ['verify', 'verify-installation'].includes(mode), 'Expected local collect/verify/verify-installation');
    const contract = verifyLocalSelection(root, mode === 'verify-installation');
    console.log(JSON.stringify({ schema: contract.schema, verified: true }));
  }
}
