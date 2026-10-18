import assert from 'node:assert/strict';
import { test } from 'node:test';
import { mkdtempSync, mkdirSync, writeFileSync, rmSync, symlinkSync, realpathSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { screenshotFile, serve } from './capture-screenshots.mjs';
function fixture(t) {
  const root = realpathSync(mkdtempSync(join(tmpdir(), 'capture-test-')));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  const dist = join(root, 'dist'); mkdirSync(dist);
  writeFileSync(join(dist, 'index.html'), 'SPA');
  writeFileSync(join(dist, 'asset.js'), 'resource');
  writeFileSync(join(root, 'secret'), 'private');
  return { root, dist };
}
for (const url of ['/../secret', '/%2e%2e/secret', '/%2e%2e%2fsecret', '/..%5csecret', '/%00', '/%zz']) {
  test(`reject unsafe raw URL ${url}`, async t => {
    const { dist } = fixture(t);
    await assert.rejects(screenshotFile(dist, url));
  });
}
test('normal resource, encoded resource and SPA fallback remain available', async t => {
  const { dist } = fixture(t);
  assert.equal(await screenshotFile(dist, '/asset.js?v=1'), join(dist, 'asset.js'));
  assert.equal(await screenshotFile(dist, '/%61sset.js'), join(dist, 'asset.js'));
  assert.equal(await screenshotFile(dist, '/nodes/example'), join(dist, 'index.html'));
  await assert.rejects(screenshotFile(dist, '/.'));
});
test('outside file/directory links and nonregular paths are rejected', { skip: process.platform === 'win32' }, async t => {
  const { dist, root } = fixture(t);
  symlinkSync(join(root, 'secret'), join(dist, 'leak'));
  symlinkSync(root, join(dist, 'outside'));
  await assert.rejects(screenshotFile(dist, '/leak'));
  await assert.rejects(screenshotFile(dist, '/outside/secret'));
  rmSync(join(dist, 'index.html'));
  symlinkSync(join(root, 'secret'), join(dist, 'index.html'));
  await assert.rejects(screenshotFile(dist, '/missing-route'));
});
test('server binds only IPv4 loopback', async t => {
  const { dist } = fixture(t);
  const server = await serve(dist, 0);
  t.after(() => new Promise(resolve => server.close(resolve)));
  assert.equal(server.address().address, '127.0.0.1');
});
