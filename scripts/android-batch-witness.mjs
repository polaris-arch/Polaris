#!/usr/bin/env node
// Prepared-only artifact verifier and offline wire-protocol fixture. No socket implementation.
import assert from 'node:assert/strict';
import { randomBytes } from 'node:crypto';
import { lstatSync, readFileSync, realpathSync } from 'node:fs';
import { basename, dirname, resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { PHASES, PROTOCOLS, expectedChallenges, privateManifestBytes,
  requireLiveExecution, sha256, validatePlan, verifyWitnessBytes } from './android-batch-qa.mjs';

export class WitnessLedger {
  constructor(plan, side, protocol) {
    validatePlan(plan);
    assert.ok(['pc', 'android'].includes(side));
    assert.ok(PROTOCOLS.includes(protocol));
    this.plan = plan;
    this.protocol = protocol;
    this.instanceSha256 = sha256(randomBytes(32));
    this.received = 0;
    this.seen = new Set();
    this.alive = true;
    const direction = side === 'pc' ? 'android-to-pc' : 'pc-to-android';
    this.allowed = new Set(PHASES.flatMap(phase => expectedChallenges(plan, phase, direction, protocol)));
  }
  receive(payload) {
    // Offline fixture only; no socket, remote process or policy action exists in this module.
    if (payload === `polaris-health-v1:${this.plan.nonce}:${this.protocol}\n`) return payload;
    this.received++;
    const value = sha256(payload);
    if (!this.allowed.has(value) || this.seen.has(value)) return null;
    this.seen.add(value);
    return payload;
  }
  snapshot(observedAtMs = Date.now()) {
    return { instanceSha256: this.instanceSha256, nonceSha256: sha256(this.plan.nonce),
      protocol: this.protocol, port: this.plan[`${this.protocol}Port`], alive: this.alive,
      received: this.received, matched: this.seen.size, challengeSha256s: [...this.seen].sort(),
      observedAtMs, dexSha256: this.plan.witnessBuild.dexSha256,
      manifestSha256: this.plan.planSha256, buildReceiptSha256: this.plan.witnessBuildReceiptSha256 };
  }
}
export function validateCounterFile(plan, path) {
  validatePlan(plan);
  const root = lstatSync(plan.qaRoot);
  assert.ok(root.isDirectory() && !root.isSymbolicLink(), 'QA root must be a real directory');
  assert.equal(root.mode & 0o777, 0o700, 'QA root must have mode 0700');
  assert.equal(root.uid, process.getuid(), 'QA root must belong to the current operator');
  assert.equal(realpathSync(plan.qaRoot), plan.qaRoot, 'QA root cannot have symlink parents');
  assert.equal(dirname(resolve(path)), plan.qaRoot, 'PC counter file must stay inside private QA root');
  assert.equal(basename(path), `pc-counters-${plan.nonce}.json`, 'PC counter file must belong to this nonce');
  assert.equal(lstatSync(path, { throwIfNoEntry: false }), undefined, 'prior PC counter file or dangling symlink cannot be reused');
  return resolve(path);
}
export function androidArgs(plan) {
  requireLiveExecution(plan); // unconditionally NOT_READY; no executable argv is emitted
}
export function probe(plan) {
  requireLiveExecution(plan);
}
export function serve(plan) {
  requireLiveExecution(plan);
}
export function checkCopiedArtifacts(plan, manifestFile, dexFile) {
  return verifyWitnessBytes(plan, readFileSync(manifestFile), readFileSync(dexFile));
}
export function main(args) {
  const [mode, planFile, approvalFile, ...options] = args;
  assert.ok(['serve', 'probe', 'android-serve', 'android-probe', 'android-health', 'verify-artifacts'].includes(mode));
  const plan = validatePlan(JSON.parse(readFileSync(planFile, 'utf8')));
  if (options.includes('--execute')) requireLiveExecution(plan);
  if (mode === 'verify-artifacts') {
    assert.equal(options.length, 1, 'verify-artifacts PLAN PRIVATE_MANIFEST DEX');
    return { mode: 'OFFLINE_ARTIFACT_VERIFICATION', ...checkCopiedArtifacts(plan, approvalFile, options[0]),
      verdict: 'NOT_READY', claims: [] };
  }
  if (mode === 'serve') {
    assert.equal(options.length, 1, 'serve requires a planned nonce-owned counter file');
    validateCounterFile(plan, options[0]);
  } else if (['probe', 'android-probe'].includes(mode)) {
    assert.ok(options.length === 2 && PHASES.includes(options[0]) && PROTOCOLS.includes(options[1]));
  } else if (mode === 'android-health') assert.ok(options.length === 1 && PROTOCOLS.includes(options[0]));
  else assert.equal(options.length, 0);
  return { mode: 'DRY_RUN', verdict: 'NOT_READY', preparedOnly: true, action: mode,
    planSha256: plan.planSha256, witnessDexSha256: plan.witnessBuild.dexSha256,
    privateManifestSha256: sha256(privateManifestBytes(plan)),
    missingObservers: plan.readiness.missingObservers, claims: [],
    note: 'No sockets or executable Android commands exist in this prepared-only module.' };
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try {
    console.log(JSON.stringify(main(process.argv.slice(2)), null, 2));
    process.exitCode = 2;
  } catch (error) {
    console.error(`FAIL: ${error.message.split('\n')[0]}`);
    process.exitCode = 1;
  }
}
