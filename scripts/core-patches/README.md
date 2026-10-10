# Desktop patched source graph

The four desktop builders consume the same core/dependency source graph through
the shared `scripts/core-source-provision.py` API. This tree consumes its exact
frozen bytes from `9929a38f19e31ce4374acbb5940bd2985df4a8c0`, SHA-256
`ef0238183e3076ed3cfa51df824cacd9a74bafa69f40298fa7aae7254e90a24d`.
That provider is owned outside this desktop slice. Existing eight Go patches
and the shared source manifest are unchanged here. Unfrozen consumer patches
and dependency pins are not imported.

`sourceBuild` in `src-tauri/core-manifest.json` pins the source manifest, the
provisioner, the patched source tree, the build tree, the module graph and each
platform's build tree. `fetch-core.mjs` rejects an incomplete set of pins before
any clone/build/resource write, including with `--force`. Old official archive
hashes, old Windows output hashes, fetch stamps and existing binaries cannot
substitute for the graph. The per-platform `binarySha256` fields and
`windowsBuild.binarySha256` are null: output digests are only required where the
caller asks for them, and the consumed binaries are bound through the producer
receipts instead. The historical `coreArchiveSha256` keys are retained only as
the list of bundled platforms; they are not proof of a patched build.

`windows-dns-refresh.patch` changes only Windows system DNS cache invalidation:
configuration reads recheck adapter DNS at most once per second, even when the
OS emits no interface event. Existing events still invalidate immediately;
unchanged configuration reuses the snapshot. No network reset or core restart.
The patch includes regression tests for DNS-only changes, restoration,
invalidation and concurrent readers. It is derived from GPLv3 sing-box source
and distributed under the same license.

The Windows patch is applied **after** the common provisioned graph. Its final
Git tree is a separate platform input pin, and its receipt includes the common
receipt plus overlay SHA. It cannot replace or drop shared patches. Native
Windows builders retain the DNS regression tests repeated 20 times; other hosts
compile those tests without executing a Windows binary.

Every platform retains the existing feature tags. Linux is linux/amd64/CGO=0
with purego and gVisor; Windows is windows/amd64/CGO=0 with purego; both macOS
architectures are darwin/CGO=1 with gVisor. macOS requires a real SDK/C compiler.
There is no CGO=0 fallback or removal of QUIC, Tailscale, naive, CCM or USB/IP.
Upstream preset drift rejects before building. These tools do not change CAP-only
TUN/P6 admission or any runtime permissions, and do not establish a cross-UID
broker contract.

Inputs and outputs have separate gates to avoid a first-build hash cycle:

- A native producer needs frozen manifest/provider digests, core/dependency
  trees, exact module inventory, declared-module graph and source receipt fingerprints,
  transport module versions, each platform's linked-module policy, version and
  four platform source trees. It does **not** need an unknown output
  hash. It verifies an explicit candidate against a clean tracked Git checkout,
  fetches fresh exact commits, calls the shared provider, applies the overlay,
  and builds with readonly go.mod, trimpath and no VCS metadata. It reads real
  `go version -m` output, checks local dependency replacements/features, and emits
  the actual binary digest and a platform receipt. The desktop BuildID binds the
  common receipt fingerprint plus final platform tree/overlay, version, Go target
  and tags plus the selected platform's patched/transport module policy;
  `go tool buildid` verifies it. Full linked module rows and main
  go.mod/go.sum byte hashes are recorded, and untracked compiler inputs reject.
  It never runs the kernel.
- After all four producer outputs are transported together, assembly computes
  a four-entry output inventory. This is a transport declaration, not an
  authentication or acceptance verdict.
- Artifact consumption requires the same candidate and all four output/receipt
  hashes, even when `--platform` selects a subset to publish. It rechecks common
  receipts, Windows overlay tree, actual embedded module replacements and build
  settings before writing any selected resource. Header paths are excluded from
  the metadata fingerprint so transport cannot change its identity.

After joint input freeze only, the explicit CLI forms are:

```sh
# On the appropriate native runner, once per platform key:
node scripts/fetch-core.mjs --producer --platform=linux \
  --candidate=FINAL_UNIFIED_SOURCE_SHA --bundle-dir=/controlled/output
# Combine outputs at <bundle>/<key>/sing-box[.exe] and their .source-receipt.json:
node scripts/fetch-core.mjs --assemble \
  --candidate=FINAL_UNIFIED_SOURCE_SHA --bundle-dir=/controlled/bundle
# Consumption defaults to all four; a selected subset still verifies all four:
node scripts/fetch-core.mjs \
  --candidate=FINAL_UNIFIED_SOURCE_SHA --bundle-dir=/controlled/bundle
```

`FINAL_UNIFIED_SOURCE_SHA` is a placeholder, not an accepted SHA. Do not replace
it with a source parent or infer artifact provenance from Git HEAD alone. Node,
Git, Python compatible with the frozen provider, and the pinned Go toolchain are
required. Provider module-cache/toolchain prerequisites remain part of its
separate contract; failure is propagated and cannot fall back to stock sources.

The source receipt must explicitly declare its scope. With dependency patches it
is `graphScope = declared-patched-modules` and `sourceGraphState =
dependencies-patched`; with none (the current manifest) it is `graphScope =
core-source-only` and `sourceGraphState = source-only`, and either mismatch
rejects. `moduleGraphQueries` holds the exact sorted declared paths. Its
`moduleGraph` and `moduleGraphSha256` cover those patched modules (an empty list
and its digest when none are declared), not the full linked graph. The provider separately binds
the full generated main files as `mainGoModSha256` and `mainGoSumSha256`.
It does not query or emit transport pins. Desktop `sourceBuild.transportPins`
is a separate inventory map from module path to exact MVS version. Every
`sourceBuild.platforms.<key>` requires two complete, disjoint classifications:
`patchedModules.requiredLinked` + `patchedModules.allowedAbsent` cover exactly
`dependencyModules`; `transportModules.requiredLinked` +
`transportModules.confirmedAbsent` cover exactly the transport inventory.
Missing fields, duplicates, overlaps, omitted inventory entries, unknown entries
or extra policy fields reject the input freeze in both JS and the Rust updater.
Every declared patched module must link on every platform with its exact upstream
version and local patched replacement: `allowedAbsent` must be empty. A linked
module carrying any replacement row must be a declared patched module. The
current manifest declares none, so `dependencyModules` and both `patchedModules`
arrays are empty and a binary with any replacement is refused. A transport
module in `requiredLinked` must be present with its exact MVS version; one in
`confirmedAbsent` must be absent. Present non-patched transports cannot use
replacements.

For example, with a common patched inventory containing `example.com/patched`,
and a transport inventory containing `example.com/transport` and
`example.com/linux-transport`, the Windows policy is:

```json
{
  "patchedModules": {
    "requiredLinked": ["example.com/patched"],
    "allowedAbsent": []
  },
  "transportModules": {
    "requiredLinked": ["example.com/transport"],
    "confirmedAbsent": ["example.com/linux-transport"]
  }
}
```

The Linux policy puts the patched and both transport modules in `requiredLinked`
with empty absence arrays. macOS freezes its own actual transport participation.
These example paths are synthetic,
not production transport pins. The selected platform policy is bound in
`platformInputFingerprint` and its BuildID; changing it rejects older platform
receipts while preserving the provider's common fingerprint and complete graph.
The final binary receipt records all linked module rows, preserves the
common receipt fingerprint as `sourceFingerprint`, and adds the final platform
inputs as `platformInputFingerprint`. Its platform BuildID is checked separately from the
provider's common BuildID contract, so the Windows overlay cannot be mistaken
for the common pre-overlay build tree.

`desktop-core.yml` runs one native producer per platform, assembles the four
outputs into a candidate bundle and uploads it; `package.yml` and
`release-risk.yml` download that exact bundle and call `fetch-core.mjs` with
`--bundle-dir` and `--candidate`. The fixtures in this directory claim no source
provenance authentication. The runtime baseline uses a common source version
only when all frozen input fields are complete; null/partial input keeps the
existing baseline and Android version behavior. Packaging uses actual consumed
source receipts/digests for frozen inputs, retaining the old Windows hash check
for the historical path. Final common source pins and NOTICE text remain pending.

Producer artifacts keep their receipt filenames visible inside each platform
artifact. Consumption stores metadata at `resources/.source-receipts/`, outside
the platform directories packaged into the app. Any future artifact upload of
that metadata must include hidden files explicitly; losing it rejects packaging
and must not disable a check. Receipt/inventory transport is not source
authentication by itself.

Light verification (no sing-box build or execution):

```sh
bash scripts/gate-node-test.sh
POLARIS_REAL_GO_BUILDINFO_TEST=1 POLARIS_BUILDINFO_GO=/path/to/pinned/go node --test \
  --test-name-pattern='real tiny local Go' scripts/build-desktop-core.test.mjs
```

Default source tests use synthetic provider receipts and a stub command runner;
they prove only rejection/control-flow logic. The opt-in requires the manifest's
pinned Go version and cross-compiles a tiny local fixture for all four targets,
with common patched modules and a Linux-only patched module. It keeps the same
go.mod replacements and confirms that Go omits the Linux-only BuildInfo row on
Windows/macOS. It downloads no dependencies and executes no output binary.
It checks actual Go row parsing, not full sing-box build provenance or macOS SDK
readiness (the tiny fixture contains only Go code).
None of these receipts prove native process exit, platform resource cleanup,
global NoOwner, managed admission or release eligibility. The existing release
freeze remains in force.

The `.polaris.N` suffix follows the upstream prerelease number so normal
version ordering remains correct: alpha.8 < alpha.8.polaris.1 < alpha.9.
Polaris builds update with the app: the app executes the core that ships inside
its installation package and has no in-app entry for updating, importing,
uploading or rolling back a core. Core directories left in the user
configuration directory by earlier versions are removed at startup and are
never executed.

To run a self-built core in the app:

- Place it in `resources/<platform>/` (`linux`, `win`, `mac-arm64` or
  `mac-x64`) and build the package with that platform's `--config`. The
  packaged app resolves the core only from its bundled resources. This is
  for local packaging: the CI packaging gates compare the packaged core with
  the pinned source build.
- In a debug build only, point `POLARIS_SINGBOX_PATH` at the binary. Release
  builds do not read this variable at all.

The app resolves the core by path. It does not verify at runtime that the file
found there is the build that matches the app version, so replacing files
inside an installed package is unsupported and unchecked.

## macOS code signature of the bundled core

Packaging never signs or otherwise rewrites the core. On the macOS legs
`scripts/macos-nested-code.sh` only checks the `sing-box` inside the app
bundle, before the bundle seal and again in the final dmg:

- it must be byte-identical to `resources/mac-<arch>/sing-box`, the file whose
  hash the source receipt records;
- its own code signature must pass `codesign --verify --strict`. One exception:
  a thin x86_64 core that `codesign` reports as not signed at all is accepted
  and logged as a notice, because Intel macOS does not require a signature to
  execute a Mach-O. A signature that is present but invalid fails on every
  architecture, and an unsigned arm64 (or universal) core fails.

A core that fails these checks fails packaging. The fix belongs to the step
that produces the core, not to packaging.

The native producer reads `go version -m` and checks the BuildID first,
then observes the actual macOS code signature before computing `binarySha256`.
It runs `codesign --verify --strict`; arm64 requires success. Thin Intel output
may remain explicitly unsigned, but an invalid signature is rejected on both
architectures. A signed core must yield exactly one actual `CDHash` from
`codesign --display --verbose=4`. The producer does not sign or rewrite the core;
it validates the Apple linker's output and fails if it does not meet this policy.

The platform output receipt binds `macCodeSignature = {state, cdHash}` in its
fingerprint: `signed` carries a lower-case 40-hex CDHash, while the thin Intel
`unsigned` exception carries `null`. These output facts do not enter the
pre-link platform BuildID, avoiding a CDHash/BuildID cycle. Non-Mac receipts
cannot carry this field. Assembly validates the state/platform policy,
fingerprint and actual bytes; native Mac consumption re-observes the signature
and compares it to the receipt. Staged copies must still match the final hash.

Before bundle sealing and again in the final dmg, the read-only core check
retains byte-for-byte `cmp` and strict signature validation, and also compares
the actual state/CDHash and byte hash with the adjacent same-source receipt in
`resources/.source-receipts`. Only the helper may be re-signed by packaging.
Native Go pins select the desktop manifest through the App's source mode;
an active desktop fork cannot accidentally select the separate mobile manifest.

These code/fixture checks do not activate a production fork manifest or certify
native output bytes. That requires real annotated consumption/upstream tags,
frozen input pins, four native producers, and final post-signature output hashes.
No guessed tag objects or empty output pins stand in for that acceptance.

Any future pin must follow the combined L/N/R/I/C source freeze and native
four-platform builds. Do not reuse an old output hash or remove the Windows
overlay until its upstream replacement and the combined graph are reviewed.
