// Fixed component/receipt transactions with byte-checked interruption recovery.
import { closeSync, fsyncSync, lstatSync, mkdirSync, openSync, readdirSync, renameSync, rmdirSync, writeFileSync } from 'node:fs';
import { basename, join, resolve } from 'node:path';
import { canonical, decode, require, sha } from './fork-input.mjs';
import { filePin, treeInventory } from './component-admission.mjs';
import { pinned } from './source-admission.mjs';

const schema = 'polaris-mobile-pair-transaction-v1';
const metadataBytes = 65536;
const entry = path => lstatSync(path, { throwIfNoEntry: false });
const equal = (left, right) => canonical(left).equals(canonical(right));
const digest = value => typeof value === 'string' && /^[0-9a-f]{64}$/.test(value);

function validPin(value) {
  if (value === null || typeof value !== 'object') return false;
  const fields = Object.keys(value).length;
  return fields === 1 && digest(value.treeSha256) ||
    fields === 2 && Number.isSafeInteger(value.bytes) && value.bytes >= 0 && digest(value.sha256);
}

function pin(path) {
  const info = entry(path);
  require(info && !info.isSymbolicLink(), 'transaction object missing or redirected');
  return info.isDirectory() ? { treeSha256: sha(canonical(treeInventory(path))) } : filePin(path);
}

function save(path, value) {
  const temporary = path + '.tmp';
  require(!entry(temporary), 'interrupted transaction metadata retained');
  const fd = openSync(temporary, 'wx', 0o600);
  try {
    const bytes = canonical(value);
    require(bytes.length <= metadataBytes, 'transaction metadata exceeds its byte budget');
    writeFileSync(fd, bytes);
    fsyncSync(fd);
  } finally { closeSync(fd); }
  renameSync(temporary, path);
}

export function recover(pairs, work) {
  const journal = join(work, 'component-transaction.json');
  if (!entry(journal)) return;
  const value = decode(pinned(journal, metadataBytes).bytes, metadataBytes);
  require(value?.schema === schema && Array.isArray(value.entries) && value.entries.length === pairs.length &&
    value.entries.every((row, index) => row?.stage === pairs[index][0] && row.destination === pairs[index][1] &&
      row.name === basename(pairs[index][1]) && validPin(row.stagePin) && (row.originalPin === null || validPin(row.originalPin))),
  'transaction paths or pins differ from caller-owned fixed slots');
  const rejected = join(work, 'rejected-pair'), previous = join(work, 'previous-pair');
  const saved = join(work, 'recovered-transaction.json');
  require(!entry(saved), 'recovered transaction record already retained');
  require([rejected, previous].every(path => !entry(path) || entry(path).isDirectory()), 'recovery directory redirected');
  for (const { destination, name, originalPin, stagePin } of value.entries.toReversed()) {
    const backup = join(previous, name), failed = join(rejected, name);
    if (entry(destination)) {
      const actual = pin(destination);
      if (originalPin !== null && !entry(backup) && equal(actual, originalPin)) continue;
      require(equal(actual, stagePin), 'unknown destination bytes; interrupted transaction preserved');
      require(!entry(failed), 'rejected object recovery slot occupied');
      mkdirSync(rejected, { recursive: true, mode: 0o700 });
      renameSync(destination, failed);
    }
    if (entry(backup)) {
      require(originalPin !== null && !entry(destination) && equal(pin(backup), originalPin),
        'original pair recovery identity differs');
      renameSync(backup, destination);
    }
    require(originalPin === null ? !entry(destination) : entry(destination) && equal(pin(destination), originalPin),
      'original pair not restored');
  }
  if (entry(previous) && readdirSync(previous).length === 0) rmdirSync(previous);
  // Retain the interrupted transaction, failed bytes and original journal.
  renameSync(journal, saved);
}

export async function install(pairs, work, validate) {
  pairs = pairs.map(([stage, destination]) => [resolve(stage), resolve(destination)]);
  work = resolve(work);
  require(new Set(pairs.map(([, destination]) => basename(destination))).size === pairs.length,
    'pair destination names collide');
  require(pairs.every(pair => pair.every(path => !entry(path)?.isSymbolicLink())), 'pair endpoint redirected');
  recover(pairs, work);
  const previous = join(work, 'previous-pair'), journal = join(work, 'component-transaction.json');
  require(!entry(previous) && !entry(join(work, 'rejected-pair')),
    'prior/rejected pair retained; verify active references before retirement');
  const entries = pairs.map(([stage, destination]) => ({
    stage, destination, name: basename(destination), stagePin: pin(stage),
    originalPin: entry(destination) ? pin(destination) : null,
  }));
  save(journal, { schema, entries });
  try {
    if (entries.some(row => row.originalPin !== null)) mkdirSync(previous, { mode: 0o700 });
    for (const { stage, destination, name, originalPin } of entries) {
      if (originalPin !== null) {
        require(equal(pin(destination), originalPin), 'old pair changed before commit');
        renameSync(destination, join(previous, name));
      }
      renameSync(stage, destination);
    }
    await validate();
    renameSync(journal, join(work, 'completed-transaction.json'));
  } catch (error) {
    try { recover(pairs, work); }
    catch (recovery) { throw new Error('interrupted component recovery retained: ' + recovery.message, { cause: error }); }
    throw error;
  }
}
