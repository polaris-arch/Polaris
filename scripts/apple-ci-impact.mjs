import { readFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

// Orthogonal to desktop package/Android selection: shared source and final
// iOS link changes require the real unsigned App+PacketTunnel build.
export function appleRequired(paths, full = false) {
  return full || paths.some((path) => {
    if (typeof path !== 'string' || path.startsWith('/') || path.includes('..') || path.includes('\\')) {
      throw new Error('Apple impact requires normalized repository paths');
    }
    return ['Cargo.toml', 'Cargo.lock', '.cargo/config.toml', 'NOTICE', 'LICENSE', 'THIRD-PARTY-LICENSES.md'].includes(path)
      || ['crates/', 'ui/', 'vendor/', 'resources/data/', 'src-tauri/src/', 'src-tauri/gen/apple/',
        'scripts/mobile-core/', 'scripts/libbox-patches/'].some((prefix) => path.startsWith(prefix))
      || /^src-tauri\/(?:Cargo\.toml|build\.rs|tauri(?:\.ios)?\.conf\.json)$/.test(path)
      || ['.github/workflows/apple.yml', '.github/workflows/release-risk.yml',
        'scripts/apple-ci-impact.mjs', 'scripts/apple-ci-impact.test.mjs',
        'scripts/apple-carrier.py', 'scripts/fetch-dashboard.mjs', 'scripts/gate-node-test.sh'].includes(path);
  });
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  console.log(appleRequired(readFileSync(0, 'utf8').split('\0').filter(Boolean), process.argv.includes('--full')));
}
