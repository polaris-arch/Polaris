import { globSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { pipeline } from 'node:stream/promises';
import { run } from 'node:test';
import { tap } from 'node:test/reporters';
import { fileURLToPath } from 'node:url';

process.chdir(fileURLToPath(new URL('../', import.meta.url)));

// Keep device scripts out of discovery and require the release contract entry.
const files = ['scripts/*.test.mjs', 'scripts/*/*.test.mjs']
  .flatMap((pattern) => globSync(pattern).sort());
const contract = join('scripts', 'assert-pc-runtime-release.test.mjs');
if (!files.includes(contract) || !statSync(contract, { throwIfNoEntry: false })?.isFile()) {
  throw new Error('Release contract test is missing');
}
const tests = run({ files, concurrency: true });
const summaries = [];
tests.on('test:summary', (summary) => {
  if (summary.file === undefined) summaries.push(summary);
});
await pipeline(tests.compose(tap), process.stdout);

if (summaries.length !== 1 || !summaries[0].success || !(summaries[0].counts.passed > 0)) {
  console.error('Release contract gate requires a successful, nonempty test run');
  process.exitCode = 1;
}
