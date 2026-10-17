// Offline identity/HTTP fixtures only; no real network, artifacts, or target code.
import assert from 'node:assert/strict';
import test from 'node:test';
import { readFileSync } from 'node:fs';
import { KERNEL_REPOSITORIES, kernelRepositoryIdentity, kernelSourceRepository, validateSourceManifest } from './source-graph.mjs';
import { publicBytes, releaseRepositoryId, validateReleaseContract } from './release-consumer.mjs';
const current = 'polaris-arch/sing-box-polaris', historical = 'polaris-arch/polaris-box';
test('kernel hosting identities are exact and do not invent release identities', () => {
  assert.deepEqual(kernelRepositoryIdentity(current), { id: 1421657509, branch: 'polaris-main' });
  assert.deepEqual(kernelRepositoryIdentity(historical), { id: 1407705262, historical: true });
  assert.equal(releaseRepositoryId({ repository: current }), 1421657509);
  assert.equal(releaseRepositoryId({ repository: historical }), 1407705262);
  for (const repo of [current + '-other', current + '/', current + '.git', '__proto__', 'SagerNet/sing-box'])
    assert.throws(() => kernelRepositoryIdentity(repo));
  for (const repo of Object.keys(KERNEL_REPOSITORIES)) assert.equal(kernelSourceRepository(`https://github.com/${repo}`), repo);
  for (const url of [`http://github.com/${current}`, `https://github.com/${current}?x=1`, `https://github.com/${current}#x`, `https://github.com/${current}/`, `https://user@github.com/${current}`])
    assert.throws(() => kernelSourceRepository(url));
});
test('source migration retains pins and historical provenance', () => {
  const source = JSON.parse(readFileSync(new URL('./source-manifest.json', import.meta.url)));
  const manifest = JSON.parse(readFileSync(new URL('../../src-tauri/core-manifest.json', import.meta.url)));
  assert.deepEqual(validateSourceManifest(source, manifest.desktopSourceBuild), []);
  assert.deepEqual(validateSourceManifest({ ...source, sourceURL: `https://github.com/${current}` }, manifest.desktopSourceBuild), []);
  assert.throws(() => validateSourceManifest({ ...source, sourceURL: `https://github.com/${current}`, sourceTree: '0'.repeat(40) }, manifest.desktopSourceBuild));
  assert.throws(() => validateReleaseContract({ schema: 'polaris-box-release-consumer-contract-v1', providerContract: 'polaris-box-release-v1', repository: current }), /unsupported/);
});
test('dedicated API credentials stay inside the selected repository', async () => {
  for (const repository of [current, historical]) {
    const observations = [];
    const fetch = async (url, options) => { observations.push(options.headers.Authorization); return new Response('ok'); };
    const options = { repository, fetch, env: { GITHUB_ACTIONS: 'true', POLARIS_RELEASE_API_TOKEN: 'fixture' } };
    await publicBytes(`https://api.github.com/repos/${repository}`, 10, false, options);
    await publicBytes(`https://api.github.com/repos/${repository === current ? historical : current}`, 10, false, options);
    await publicBytes(`https://github.com/${repository}/releases/download/inert/core.zip`, 10, true, options);
    assert.deepEqual(observations, ['Bearer fixture', undefined, undefined]);
  }
});
