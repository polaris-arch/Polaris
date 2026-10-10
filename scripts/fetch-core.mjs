#!/usr/bin/env node
/** Produce one native source artifact, or consume an exact four-artifact bundle.
 * Consumption defaults to all four; producer selection must be explicit.
 * Darwin CGO needs a real SDK/compiler. Incomplete sourceBuild inputs reject
 * before provisioning. --force remains accepted, with no old-cache/stock bypass.
 * This command never runs a target kernel; native Windows runs DNS unit tests.
 */
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { produceDesktopCore } from './desktop-core/build-core.mjs';
import { consumeDesktopBundle, coreFilename, writeBundleInventory } from './desktop-core/bundle.mjs';
import { writeValidationBundle } from './desktop-core/validation-origin.mjs';
import { validateSourcePins } from './desktop-core/source-graph.mjs';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const manifest = JSON.parse(readFileSync(join(ROOT, 'src-tauri/core-manifest.json'), 'utf8'));
const TARGETS = [
  { dir: 'resources/linux', bin: 'sing-box', key: 'linux' },
  { dir: 'resources/win', bin: 'sing-box.exe', key: 'win' },
  { dir: 'resources/mac-x64', bin: 'sing-box', key: 'mac-x64' },
  { dir: 'resources/mac-arm64', bin: 'sing-box', key: 'mac-arm64' },
];

function selectedKeys() {
  const arg = process.argv.find((a) => a.startsWith('--platform='));
  if (!arg) return null; // 全平台
  const all = TARGETS.map((t) => t.key);
  const want = arg.slice('--platform='.length).split(',').map((s) => s.trim()).filter(Boolean);
  if (want.length === 0) {
    console.error(`FAILED: --platform 值为空。可选：${all.join(' / ')}（不传该 flag = 全平台）`);
    process.exit(1);
  }
  const bad = want.filter((k) => !all.includes(k));
  if (bad.length > 0) {
    console.error(`FAILED: 未知平台 ${bad.join(', ')}。可选：${all.join(' / ')}`);
    process.exit(1);
  }
  return want;
}

const ONLY = selectedKeys();
const PICKED = ONLY ? TARGETS.filter((target) => ONLY.includes(target.key)) : TARGETS;
function valueOf(name) {
  const args = process.argv.slice(2).filter((arg) => arg.startsWith(`--${name}=`));
  if (args.length > 1) throw new Error(`Duplicate --${name}`);
  return args[0]?.slice(name.length + 3);
}
try {
  validateSourcePins(manifest, PICKED[0].key, false);
  const candidate = valueOf('candidate');
  const bundle = valueOf('bundle-dir');
  if (process.argv.includes('--producer')) {
    if (!ONLY || ONLY.length !== 1 || !bundle || process.argv.includes('--assemble')) {
      throw new Error('Producer requires exactly one explicit --platform and --bundle-dir; do not fake cross-platform native builds');
    }
    const key = ONLY[0];
    const receipt = produceDesktopCore(ROOT, manifest, key, join(bundle, key, coreFilename(key)), candidate);
    console.log(JSON.stringify({ candidate, platform: key, binarySha256: receipt.binarySha256, fingerprint: receipt.fingerprint }));
  } else if (process.argv.includes('--assemble')) {
    if (!bundle || ONLY) throw new Error('Assembly requires --bundle-dir and all four producer outputs');
    if (process.argv.includes('--validation-origin')) writeValidationBundle(ROOT, bundle, candidate);
    else writeBundleInventory(bundle, candidate);
  } else {
    if (!bundle) throw new Error('Consumption requires --bundle-dir containing all four candidate producer outputs');
    consumeDesktopBundle(ROOT, manifest, bundle, candidate, PICKED.map((target) => target.key));
  }
} catch (error) {
  console.error(`FAILED desktop source graph: ${error.message}`);
  process.exitCode = 1;
}
