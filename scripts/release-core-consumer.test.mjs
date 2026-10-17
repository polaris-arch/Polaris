// Keep the release consumer inside the existing scripts/*.test.mjs CI collection.
import './desktop-core/release-consumer.test.mjs';
import './desktop-core/upstream-release.test.mjs';
import { execFileSync } from 'node:child_process';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

test('native ZIP/tar safety boundary rejects private adversarial archive fixtures', () => {
  execFileSync(process.platform === 'win32' ? 'python' : 'python3', ['-B',
    fileURLToPath(new URL('./desktop-core/test-release-archive.py', import.meta.url))], { stdio: 'pipe' });
});
