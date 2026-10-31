// Build-time preservation of the selected original archive, separate from run resources.
// Runtime code consumes the compiled selection only on an explicit recovery request.
import { chmodSync, constants, copyFileSync, lstatSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { canonical, digest, requireGraph as demand } from './source-graph.mjs';

export const RECOVERY_RESOURCE = 'core-recovery';
export const RECOVERY_INVENTORY = 'inventory.json';
const MAX_MEMBER_BYTES = 200 * 1024 * 1024;
const MAX_EXPANDED_BYTES = 1024 ** 3;
const hex = value => typeof value === 'string' && /^[a-f0-9]{64}$/.test(value);
const same = (a, b) => canonical(a) === canonical(b);

function regular(path, directory = false) {
  const st = lstatSync(path);
  demand(directory ? st.isDirectory() : st.isFile() && st.nlink === 1, 'Linked or non-regular recovery resource');
  for (let at = dirname(path); ; at = dirname(at)) {
    demand(lstatSync(at).isDirectory(), 'Linked recovery resource ancestor');
    if (at === dirname(at)) break;
  }
  return st;
}

function archivePin(row) {
  const { name, bytes, sha256 } = row.archive;
  demand(typeof name === 'string' && /^[A-Za-z0-9_+.-]+$/.test(name)
    && Number.isSafeInteger(bytes) && bytes > 0 && bytes <= MAX_MEMBER_BYTES && hex(sha256), 'Invalid recovery archive pin');
  const format = name.endsWith('.tar.gz') ? 'tar.gz' : name.endsWith('.zip') ? 'zip' : null;
  const names = Object.keys(row.archive.members ?? {}).sort();
  const roots = new Set(names.map(n => n.split('/')[0]));
  demand(format && names.length > 0 && names.length <= 1024 && roots.size === 1, 'Invalid recovery archive inventory');
  const root = [...roots][0];
  demand(names.every(n => n.startsWith(root + '/') && n.split('/').every(p => /^[A-Za-z0-9_+.-]+$/.test(p) && !['.', '..'].includes(p))), 'Unsafe recovery member');
  return { name, format, bytes, sha256, root };
}

function memberSizes(row, locate) {
  const pin = archivePin(row), sizes = {}; let total = 0;
  for (const name of Object.keys(row.archive.members).sort()) {
    const relative = name.slice(pin.root.length + 1), path = locate(relative);
    const st = regular(path);
    demand(st.size <= MAX_MEMBER_BYTES && digest(readFileSync(path)) === row.archive.members[name], 'Recovery source member differs');
    sizes[name] = st.size; total += st.size;
    demand(total <= MAX_EXPANDED_BYTES, 'Recovery expanded byte budget');
  }
  return sizes;
}

export function stageRecoveryResource(directory, row, platform, contractSha256, archive, extractedRoot) {
  demand(['linux', 'win', 'mac-x64', 'mac-arm64'].includes(platform) && hex(contractSha256), 'Unbound recovery selection');
  const pin = archivePin(row);
  demand(regular(archive).size === pin.bytes && digest(readFileSync(archive)) === pin.sha256, 'Recovery compressed bytes differ');
  const inventory = { schema: 'polaris-core-recovery-inventory-v1', platform, contractSha256,
    archive: pin, memberBytes: memberSizes(row, relative => join(extractedRoot, relative)) };
  mkdirSync(directory, { recursive: true }); regular(directory, true);
  demand(readdirSync(directory).length === 0, 'Recovery staging must be empty');
  const copied = join(directory, pin.name);
  copyFileSync(archive, copied, constants.COPYFILE_EXCL);
  // Only our new copy in the existing private publication stage is changed.
  // Node's NTFS facade cannot express Unix owner/group bits; its chmod still
  // sets the data-file writable projection. Native ACL proof is a later gate.
  chmodSync(copied, 0o644);
  demand(process.platform === 'win32' || (regular(copied).mode & 0o7777) === 0o644,
    'Recovery archive data mode differs');
  demand(digest(readFileSync(copied)) === pin.sha256, 'Recovery archive changed during copy');
  writeFileSync(join(directory, RECOVERY_INVENTORY), JSON.stringify(inventory, null, 2) + '\n', { flag: 'wx', mode: 0o644 });
  return inventory;
}

// Production/package verification only. This is never called by normal Start.
export function verifyRecoveryResource(directory, row, platform, contractSha256, payloadDirectory) {
  const pin = archivePin(row); regular(directory, true);
  demand(same(readdirSync(directory).sort(), [pin.name, RECOVERY_INVENTORY].sort()), 'Extra/missing recovery resource');
  const archive = join(directory, pin.name), file = join(directory, RECOVERY_INVENTORY);
  demand(regular(archive).size === pin.bytes && digest(readFileSync(archive)) === pin.sha256, 'Recovery compressed bytes differ');
  demand(regular(file).size <= 1024 * 1024, 'Recovery inventory budget');
  const inventory = JSON.parse(readFileSync(file, 'utf8'));
  const expected = { schema: 'polaris-core-recovery-inventory-v1', platform, contractSha256, archive: pin,
    memberBytes: memberSizes(row, relative => join(payloadDirectory,
      [row.core.member, row.sidecar?.member].includes(relative) ? relative : 'core-release/' + relative)) };
  demand(same(inventory, expected), 'Recovery inventory differs from selected archive');
  return inventory;
}
