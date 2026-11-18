import { appendFileSync, lstatSync, readFileSync } from 'node:fs';
import { join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { parseArgs } from 'node:util';
import { loadReleaseContract, regularPath } from '../desktop-core/release-consumer.mjs';
import { digest, releaseGoVersion, requireGraph } from '../desktop-core/source-graph.mjs';

const root = fileURLToPath(new URL('../../', import.meta.url));

function stableVersion(value) {
  const match = /^1\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)$/.exec(value);
  requireGraph(match, 'canonical exact stable Go patch version required');
  return match.slice(1).map(BigInt);
}

function atLeast(candidate, minimum) {
  return candidate[0] > minimum[0] || candidate[0] === minimum[0] && candidate[1] >= minimum[1];
}

try {
  const { values } = parseArgs({ options: {
    'kernel-root': { type: 'string' }, mode: { type: 'string' }, source: { type: 'string' },
    'actual-go': { type: 'string' }, 'github-output': { type: 'string' },
  } });
  const { 'kernel-root': kernelRoot, mode, source, 'actual-go': actualGo, 'github-output': output } = values;
  requireGraph(kernelRoot && mode === 'formal' && /^[0-9a-f]{40}$/.test(source),
    'explicit mode, kernel root and exact source required');
  const manifest = JSON.parse(readFileSync(join(root, 'src-tauri/core-manifest.json')));
  const contract = loadReleaseContract(root, manifest);
  const row = contract.platforms.win;
  requireGraph(contract.sourceCommit === source && row.sourceCommit === source, 'formal source differs');

  const moduleFile = join(kernelRoot, 'go.mod');
  regularPath(moduleFile);
  requireGraph(lstatSync(moduleFile).size <= 2 * 1024 ** 2, 'bounded source go.mod required');
  const raw = readFileSync(moduleFile);
  requireGraph(digest(raw) === row.sourceIdentity.goModSha256, 'exact released source go.mod differs');
  // With the full original file admitted, read only the declarations needed
  // for this compiler choice. The released source already passed Go parsing.
  const directives = Object.fromEntries([...raw.toString('utf8')
    .matchAll(/^(module|go|toolchain)[ \t]+([^\s]+)[ \t]*(?:\/\/.*)?$/gm)]
    .map(([, key, value]) => [key, value]));
  requireGraph(directives.module === 'github.com/sagernet/sing-box', 'kernel module differs');
  const minimum = stableVersion(directives.go);
  const suggested = directives.toolchain ?? null;
  const compiler = releaseGoVersion(contract);
  requireGraph(compiler === row.goVersion, 'formal compiler fact differs');
  const selected = stableVersion(compiler.slice(2));
  requireGraph(atLeast(selected, minimum), 'formal compiler below Go minimum');
  if (suggested !== null) {
    requireGraph(suggested.startsWith('go'), 'standard Go toolchain required');
    const toolchain = stableVersion(suggested.slice(2));
    requireGraph(atLeast(toolchain, minimum), 'toolchain is below Go minimum');
    requireGraph(atLeast(selected, toolchain), 'formal compiler conflicts with source suggestion');
  }
  requireGraph(actualGo === undefined || actualGo === compiler, 'actual Go compiler differs from selected exact compiler');
  if (output) appendFileSync(output, 'version=' + compiler.slice(2) + '\n');
  console.log(JSON.stringify({ validationMode: mode, kernelSource: source, goMinimum: directives.go,
    sourceToolchain: suggested, selectedCompiler: compiler, selectionOrigin: 'formal-release-producer-fact',
    actualCompiler: actualGo ?? null }));
} catch (error) {
  console.error(error.message);
  process.exitCode = 1;
}
