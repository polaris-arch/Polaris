import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync, existsSync, readdirSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join, dirname } from 'node:path';
import { installCache, completeCache } from './fetch-protoc.mjs';
const protos = ['any', 'api', 'descriptor', 'duration', 'empty', 'field_mask', 'source_context', 'struct', 'timestamp', 'type', 'wrappers'];
function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), 'protoc-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  return { root, dest: join(root, 'cache') };
}
function extract(_zip, dir) {
  for (const name of ['bin/protoc', ...protos.map(n => `include/google/protobuf/${n}.proto`)]) {
    mkdirSync(dirname(join(dir, name)), { recursive: true });
    writeFileSync(join(dir, name), name);
  }
}
test('partial extraction never publishes cache and cleans sibling staging', t => {
  const { root, dest } = fixture(t);
  assert.throws(() => installCache('fixture', dest, (zip, dir) => { extract(zip, dir); throw Error('halfway'); }, 'linux'), /halfway/);
  assert.equal(existsSync(dest), false);
  assert.deepEqual(readdirSync(root), []);
});
for (const missing of ['bin/protoc', 'include/google/protobuf/descriptor.proto']) {
  test(`missing required ${missing} refuses publication`, t => {
    const { dest } = fixture(t);
    assert.throws(() => installCache('fixture', dest, (zip, dir) => { extract(zip, dir); rmSync(join(dir, missing)); }, 'linux'), /missing protoc member/);
    assert.equal(completeCache(dest, 'linux'), false);
  });
}
test('complete cache hits, but missing, changed or extra members invalidate it', t => {
  const { dest } = fixture(t);
  installCache('fixture', dest, extract, 'linux');
  assert.equal(completeCache(dest, 'linux'), true);
  writeFileSync(join(dest, 'extra'), 'extra');
  assert.equal(completeCache(dest, 'linux'), false);
  rmSync(join(dest, 'extra'));
  writeFileSync(join(dest, 'include/google/protobuf/any.proto'), 'corrupt');
  assert.equal(completeCache(dest, 'linux'), false);
});
test('failed forced refresh preserves previous complete installation', t => {
  const { dest } = fixture(t);
  installCache('fixture', dest, extract, 'linux');
  const old = readFileSync(join(dest, '.polaris-complete.json'), 'utf8');
  assert.throws(() => installCache('fixture', dest, () => { throw Error('extract'); }, 'linux'));
  assert.equal(readFileSync(join(dest, '.polaris-complete.json'), 'utf8'), old);
  assert.equal(completeCache(dest, 'linux'), true);
});
test('legacy bin-only cache is not a completion marker', t => {
  const { dest } = fixture(t);
  mkdirSync(join(dest, 'bin'), { recursive: true });
  writeFileSync(join(dest, 'bin/protoc'), 'partial');
  assert.equal(completeCache(dest, 'linux'), false);
});
