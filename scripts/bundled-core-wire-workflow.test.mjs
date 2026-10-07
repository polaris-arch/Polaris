import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const command = 'cargo test -p polaris-singbox-grpc --test bundled_core_wire vendored_proto_matches_every_bundled_core -- --exact';
const wireStep = 'Bundled core gRPC wire contract (all platforms)';

function step(yaml, name) {
  const lines = yaml.split('\n');
  const start = lines.findIndex((line) => line === `      - name: ${name}`);
  assert.ok(start >= 0, `missing workflow step: ${name}`);
  const next = lines.findIndex((line, index) => index > start && /^      - (?:name:|uses:)/.test(line));
  return { start, body: lines.slice(start, next < 0 ? lines.length : next).join('\n') };
}

function assertWireStep(yaml, { kernelOnly }) {
  const { body } = step(yaml, wireStep);
  assert.match(body, /^        env:\n          POLARIS_REQUIRE_KERNEL_GATE: '1'$/m);
  assert.ok(body.includes(`        run: ${command}\n`), 'must run only the exact descriptor test');
  assert.equal(body.includes('POLARIS_NO_KERNEL_RUN'), false, 'the static wire gate does not inherit the local no-kernel-run mode');
  if (kernelOnly) assert.match(body, /^        if: needs\.classify\.outputs\.kernel == 'true'$/m);
  else assert.doesNotMatch(body, /^        if:/m, 'direct Package dispatch must not skip the wire gate');
}

function assertAllFourSourceConsumption(fetch) {
  assert.match(fetch.body, /^        run: node scripts\/fetch-core\.mjs --bundle-dir="\$CORE_BUNDLE" --candidate="\$CORE_CANDIDATE"$/m);
  assert.match(fetch.body, /^          CORE_BUNDLE: \$\{\{ runner.temp \}\}\/desktop-core-bundle$/m);
  assert.doesNotMatch(fetch.body, /--platform=|--force|continue-on-error|\|\| true/);
}

test('Package checks all fetched cores before optional runtime gates', () => {
  const yaml = readFileSync(join(root, '.github/workflows/package.yml'), 'utf8');
  assertWireStep(yaml, { kernelOnly: false });
  const protoc = step(yaml, 'Install protoc (pinned URL + sha256)');
  const fetch = step(yaml, 'Consume sing-box core bundle');
  const wire = step(yaml, wireStep);
  const optionalRuntime = step(yaml, 'Bundled core dependency fingerprint (sing-tun pin)');
  assert.match(protoc.body, /^        run: node scripts\/fetch-protoc\.mjs$/m);
  assertAllFourSourceConsumption(fetch);
  assert.ok(protoc.start < fetch.start && fetch.start < wire.start && wire.start < optionalRuntime.start);
});

test('Release Risk checks all fetched cores only on kernel-impact changes', () => {
  const yaml = readFileSync(join(root, '.github/workflows/release-risk.yml'), 'utf8');
  assertWireStep(yaml, { kernelOnly: true });
  const fetch = step(yaml, 'Fetch sing-box core');
  const rust = step(yaml, 'Install Rust stable for bundled-core gates');
  const protoc = step(yaml, 'Fetch pinned protoc for gRPC wire check');
  const wire = step(yaml, wireStep);
  assertAllFourSourceConsumption(fetch);
  assert.match(protoc.body, /^        if: needs\.classify\.outputs\.kernel == 'true'$/m);
  assert.match(protoc.body, /^        run: node scripts\/fetch-protoc\.mjs$/m);
  assert.ok(fetch.start < rust.start && rust.start < protoc.start && protoc.start < wire.start);
});
