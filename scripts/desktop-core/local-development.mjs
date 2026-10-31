#!/usr/bin/env node
// The command chooses a producer operation, never a runtime trust lane.
import { execFileSync } from 'node:child_process';
import { readFileSync, existsSync, lstatSync, openSync, fstatSync, readSync, closeSync, constants, realpathSync } from 'node:fs';
import { resolve, dirname, join } from 'node:path';
import { fileURLToPath, pathToFileURL } from 'node:url';
import { selectedCoreManifest, verifySelectedCoreManifest, actualCoreFile, writeSelectedCoreManifest } from '../lib/core-identity.mjs';
import { requireGraph as demand } from './source-graph.mjs';
export const LOCAL_MARKER = Buffer.from('POLARIS_SELECTION_LOCAL_DEVELOPMENT_V1');
const root = resolve(dirname(fileURLToPath(import.meta.url)), '../..');
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

export function verifyLocalSelection(base = root, installation = false) {
  const directory = localDirectory(base);
  // Fixed code and fixed input slot. No env selector or receipt chooses Cargo's lane.
  execFileSync('python3', [join(base, 'scripts/desktop-core/local-development.py'), 'verify'], { cwd: base, env: { ...process.env, GOENV: 'off', GOTOOLCHAIN: 'local', GOWORK: 'off', GOPROXY: 'off', GOSUMDB: 'off', PYTHONDONTWRITEBYTECODE: '1' }, stdio: ['ignore', 'pipe', 'pipe'], maxBuffer: 2 * 1024 ** 2 });
  const contract = JSON.parse(readFileSync(join(directory, 'contract.json')));
  demand(contract.schema === 'polaris-local-development-core-contract-v1' && Object.keys(contract.platforms).join() === 'linux', 'Local contract is Linux-only');
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
  } else {
    demand(args.length === 0 && ['verify', 'verify-installation'].includes(mode), 'Expected local collect/verify/verify-installation');
    const contract = verifyLocalSelection(root, mode === 'verify-installation');
    console.log(JSON.stringify({ schema: contract.schema, verified: true }));
  }
}
