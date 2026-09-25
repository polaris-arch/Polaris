# Windows DNS refresh build

Windows ships the pinned `windowsBuild` in `src-tauri/core-manifest.json`.
Linux and macOS keep the official `bundledCoreVersion` release assets.

`windows-dns-refresh.patch` changes only Windows system DNS cache invalidation:
configuration reads recheck adapter DNS at most once per second, even when the
OS emits no interface event. Existing events still invalidate immediately;
unchanged configuration reuses the snapshot. No network reset or core restart.
The patch includes regression tests for DNS-only changes, restoration,
invalidation and concurrent readers. It is derived from GPLv3 sing-box source
and distributed under the same license.

Build: `node scripts/fetch-core.mjs --platform=win --force` (Node, Git, curl,
tar and Go required). The manifest pins the upstream source archive, patch,
Go toolchain and output SHA-256. Builds use the upstream Windows tags/flags,
CGO disabled, trimpath and no VCS build metadata. Windows builders execute
the regression tests 20 times; other hosts cross-compile them without running
the kernel. A mismatched output is never installed in resources.

The `.polaris.N` suffix follows the upstream prerelease number so normal
version ordering remains correct: alpha.8 < alpha.8.polaris.1 < alpha.9.
Polaris builds update with the app. Official online core updates (including
already staged downloads) cannot replace them. Explicit manual imports stay
available and retain the existing manual-core protection.

To update: review the upstream patch applicability, rebuild with pinned inputs,
update all hashes and the NOTICE version, then run native Windows tests and
package validation. Do not reuse an old binary hash or bypass the check.
When upstream includes the fix, remove `windowsBuild` and the patch/build
branch; a newer official bundled version replaces the old managed build via
the ordinary version comparison.
