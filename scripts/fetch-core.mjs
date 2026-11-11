#!/usr/bin/env node
/** Acquire the committed desktop release archives into resources/.
 * Defaults to all four platforms; `--platform=a,b` selects a subset.
 * `--recover-release` restores an interrupted acquisition. --force remains
 * accepted. This command never runs a target kernel.
 */
import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { consumeRelease, recoverRelease, releasePlatforms } from './desktop-core/release-consumer.mjs';

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..');
const manifest = JSON.parse(readFileSync(join(ROOT, 'src-tauri/core-manifest.json'), 'utf8'));
try {
  const argv = process.argv.slice(2);
  if (argv.length === 1 && argv[0] === '--recover-release') {
    console.log(JSON.stringify(recoverRelease(ROOT)));
  } else {
    await consumeRelease(ROOT, manifest, releasePlatforms(argv));
    console.log('Pinned release resources ready; native App acceptance remains pending');
  }
} catch (error) {
  console.error(`FAILED desktop release acquisition: ${error.message}`);
  process.exitCode = 1;
}
