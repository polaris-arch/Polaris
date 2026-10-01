// Preserve the Windows DNS overlay validation; build only the combined graph.
import { join } from 'node:path';
import { buildDesktopCore } from '../desktop-core/build-core.mjs';
import { verifyHash } from '../desktop-core/source-graph.mjs';
export { verifyHash } from '../desktop-core/source-graph.mjs';

export function buildWindowsCore(root, manifest, dest, force, run, candidate) {
  const spec = manifest.windowsBuild;
  if (!spec || !/^[a-f0-9]{40}$/.test(spec.sourceCommit)
      || !/^\d+\.\d+\.\d+$/.test(spec.goVersion)
      || !/^[a-f0-9]{64}$/.test(spec.binarySha256 ?? '')
      || !spec.version?.startsWith(`${manifest.bundledCoreVersion}.polaris.`)
      || !/^[1-9][0-9]*$/.test(spec.version.slice(`${manifest.bundledCoreVersion}.polaris.`.length))) {
    throw new Error('Invalid pinned Windows build manifest');
  }
  verifyHash(join(root, 'scripts/core-patches/windows-dns-refresh.patch'), spec.patchSha256);
  return buildDesktopCore(root, manifest, 'win', dest, force, run, { candidate });
}
