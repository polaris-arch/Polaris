# Build and Package

<div align="center">

[简体中文](build-and-package.md) · **English**

</div>

Split out of the README (2026-08-13): this is engineering internals and invariants, written for maintainers, not end users. The README keeps only "how to install, how to use".

## Build

Implementation lands in batches per system design §H (B0 scaffolding → B10 release engineering).

### Toolchain requirements

| Tool | Version | Purpose |
|---|---|---|
| Rust | stable (edition 2021) | Backend + domain crates under `crates/` (plus `source-probe`, which is dev-only: it appears solely in `[dev-dependencies]` and never in a lib/bin dependency graph) |
| Node.js | 24+ (CI pins 26) | Frontend build + fetch scripts |
| pnpm | 11.24.0 (pinned by `ui/package.json`) | Frontend package management (`ui/`); the `beforeBuildCommand` of `tauri build` calls `pnpm` directly, so it must be on PATH |
| Go | `goVersion` in the source manifest for the selected build path | Source builds and binary build-information inspection; desktop and mobile inputs are separate |
| [Tauri CLI 2](https://v2.tauri.app/) | Pinned by `scripts/tauri-cli.version` | `tauri build` packaging with the global CLI |

Install the global CLI from the repository root. The CLI is no longer a `ui/` devDependency; desktop and Android CI install the same pinned version.

```bash
npm install -g "@tauri-apps/cli@$(cat scripts/tauri-cli.version)"
tauri --version
```

### System dependencies

**Linux** (Tauri 2 WebKitGTK 4.1 stack, Debian/Ubuntu):

```bash
sudo apt install libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev \
  librsvg2-dev libglib2.0-dev libsoup-3.0-dev libjavascriptcoregtk-4.1-dev \
  libdbus-1-dev pkg-config
```

**macOS**: 13.0+ (Ventura, required by the three-tier BTM "allow in background" probe), plus Xcode Command Line Tools.
**Windows**: MSVC build tools (Visual Studio Build Tools 2022 + Windows 10/11 SDK).

### Rust workspace (development gates)

```bash
cargo build --workspace        # compile
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace         # unit tests
cargo fmt --all -- --check     # formatting
```

### Frontend toolchain and AST tests

The frontend currently uses React 19, TypeScript 7, Vite 8, Vitest 4, and Tailwind CSS 4.
Install, build, and test with the pnpm 11.24.0 version pinned by `ui/package.json`:

```bash
cd ui
npx pnpm@11.24.0 install --frozen-lockfile
npx pnpm@11.24.0 run build
npx pnpm@11.24.0 test
```

TypeScript 7 no longer uses the legacy
`createSourceFile(..., ScriptTarget, ..., ScriptKind)` test entry point. Structural source tests go
through `ui/src/test/ts-compiler.ts`, backed by the native `typescript/unstable/sync`,
`typescript/unstable/fs`, and `typescript/unstable/ast` APIs. Call sites use only
`parseSourceFile(fileName, text)`; new AST gates must not reintroduce the old compiler call shape or
create a second compatibility layer.

### Fetching assets (sing-box core / cronet / dashboard)

Current desktop packaging starts from `desktopRelease` in `src-tauri/core-manifest.json` and consumes the pinned
[polaris-box](https://github.com/polaris-arch/polaris-box) release through `scripts/desktop-core/release-contract.json`.
The source commit, tag object, asset index, and digests are pinned. Do not substitute the fork's latest default-branch
commit, the historical `sourceBuild`, or the mobile `scripts/libbox-patches/source-manifest.json` for this desktop selection.
These build inputs do not establish native App acceptance or publication of an App candidate.

```bash
# Prepare local Linux resources; other platforms: win, mac-x64, mac-arm64
node scripts/fetch-core.mjs --platform=linux
node scripts/fetch-cronet.mjs --platform=linux
node scripts/fetch-dashboard.mjs
```

`fetch-core` verifies the pinned release inputs and places resources in `resources/<platform>/`. Linux / Windows
Cronet libraries must also match their manifest digests; macOS uses static inputs. `fetch-dashboard` obtains the
sing-box `gh-pages` dashboard and currently has no SHA256 pin. Resources are not committed and must be prepared
before packaging. This build-time fetch is not an in-app online core replacement feature.

#### Release packaging and native acceptance bundles

`.github/workflows/desktop-core.yml` selects its path from the frozen manifest: release-selection mode verifies pinned
assets on four native platforms; source mode builds from pinned source. The native inputs form a candidate-specific
bundle, consumed by `package.yml` as follows:

```bash
node scripts/fetch-core.mjs --bundle-dir=<core-bundle-dir> --candidate="$(git rev-parse HEAD)"
```

Supply `--bundle-dir` and `--candidate` together. The current `desktopRelease` path verifies the native acceptance bundle
before consuming the pinned assets. A local single-platform download does not replace release acceptance. Use the actual
artifact name and candidate commit returned by the workflow, never receipts for another commit. The current manifest has
`desktopRelease`; do not use the old `--producer` example directly. Source production applies only without that selection.

`tauri.conf.json` has no `build.beforeBundleCommand`; `tauri build` does not prepare these resources for you.
After fetching, CI runs `node scripts/verify-packaging.mjs confs` to check that resources exist and are nonempty.
Payload / asset checks and native acceptance are still required. A successful download, digest verification, and runtime
acceptance are separate checks.

To change the desktop core selection, maintainers must review `desktopRelease`, the release contract and its digest,
and the actual pinned release assets together. Mobile source manifests and historical source-mode inputs are maintained
separately. Changing only `bundledCoreVersion` or the old `sourceBuild` does not update the desktop selection.
The core ships with the App; this guide does not expose independent user core upload, online replacement, or rollback.

### Producing installers

The artifact names, asset tables, and three Android APKs described here are the build and release conventions of the current `main` source, for maintainers preparing candidate packages. They are not the current public download list and do not establish that a candidate has been published or passed installation acceptance. For published downloads, use the [README installation guide](../README.en.md#install) and the actual assets on [v1.0.0 Release](https://github.com/polaris-arch/Polaris/releases/tag/v1.0.0).

```bash
# 1) Fetch assets (see above)
# 2) Frontend build + Rust compile + installer (Tauri CLI orchestrates beforeBuildCommand)
#    Run from the **repository root** and pass this platform's config explicitly (see "Per-platform core filtering")
tauri build --config src-tauri/tauri.linux.conf.json          # Linux
tauri build --config src-tauri/tauri.windows.conf.json        # Windows
tauri build --config src-tauri/tauri.macos-arm64.conf.json    # macOS Apple Silicon
tauri build --config src-tauri/tauri.macos-x64.conf.json --target x86_64-apple-darwin  # macOS Intel
```

Artifacts land in `target/release/bundle/` at the **repository root** (this repo is a cargo workspace whose root is the repository root, so **not** `src-tauri/target/`). When `--target <triple>` is passed, they go one level deeper: `target/<triple>/release/bundle/`.

| Platform | Artifact | Form |
|---|---|---|
| Linux | `Polaris_<version>_amd64-linux.deb` / `Polaris_<version>_amd64-linux.AppImage` | deb package + AppImage (single file, no install) |
| macOS | `Polaris_<version>_aarch64-mac.dmg` / `Polaris_<version>_x64-mac.dmg` | **One per architecture** (no universal build any more, unsigned). ⚠️ These are **release asset names**, not local artifact names — see below |
| Windows | `Polaris_<version>_x64-win-setup.exe` | NSIS installer (WebView2 downloadBootstrapper, Runtime not embedded) |
| Windows | `Polaris_<version>_x64-win-Portable.zip` | Portable build (extract and run; ships its own `resources/` plus a `portable.marker` form marker) |

The portable zip is produced by the Windows leg of `package.yml` from `target/release/polaris.exe` plus `resources/`; running the `tauri build` command above locally does not produce it.

Public names follow `Polaris_<version>_<architecture>-<platform>[-<form>].<extension>`.
`package.yml` normalizes macOS, Linux and Windows names before artifact verification. Local `tauri build`
still produces Tauri's default names. Versions come from `src-tauri/tauri.conf.json`, never a CI run label.
The desktop draft requires six assets. The complete release requires those six, the three Android APKs
below, and `SHA256SUMS`. `scripts/release-assets.mjs` defines the names and inventory;
`verify-packaging.mjs assets --label release-all` checks the full set, version, size and every checksum.
Old-name copies and extra files are rejected.

### Android signed release

| ABI | Public name |
|---|---|
| ARMv8 | `Polaris_<version>_arm64-v8a-android.apk` |
| ARMv7 | `Polaris_<version>_armeabi-v7a-android.apk` |
| ARMv8 + ARMv7 | `Polaris_<version>_universal-android.apk` |

After desktop draft checks, `android.yml` builds three signed release APKs:

```bash
bash scripts/build-android-apk.sh --apk --split-per-abi --target aarch64 armv7 --ci --config src-tauri/tauri.android.conf.json
bash scripts/build-android-apk.sh --apk --target aarch64 armv7 --ci --config src-tauri/tauri.android.conf.json
```

These build `arm64Release` / `armRelease` and `universalRelease`. Every flavor uses the full Rust release
configuration, R8, resource shrinking, compressed native libraries and safe stripping on copied native
outputs. Both commands target only `arm64-v8a` and `armeabi-v7a`; release JNI packaging also excludes
x86/x86_64 libraries without widening either split flavor's ABI set. Universal contains both ARM ABIs.
Debug development builds retain emulator ABIs. Each APK is checked for manifest versions/application ID,
its exact actual ABI set, each ELF's architecture/symbols/compression, R8 evidence, source AAR bytes and
the pinned official signing certificate. Missing signing credentials fail closed. Unsigned light risk-gate
APKs cannot be released. The combined manifest binds draft downloads to the Android job's fixed verified
SHA values; the full remote digest set is checked again immediately before publication.
There is no iOS release asset. Actual builds, signatures, package sizes and installation remain candidate
release acceptance work.

#### Per-platform core filtering (`--config` is not optional)

`bundle.resources` in `src-tauri/tauri.conf.json` **contains no platform core directory at all**; the four platform cores are specified by `tauri.{linux,windows,macos-arm64,macos-x64}.conf.json`. The reasoning and the discipline:

- Bundling all four cores would add roughly 210 MB of dead weight to every package (at runtime only one is selected, by `env::consts::OS/ARCH`).
- Merging follows RFC 7396, where **arrays are replaced wholesale rather than merged**, so any shared resource added to the base config must be mirrored into all four files — otherwise all four packages silently lose it.
- **Do not rely on Tauri's implicit per-platform-name merging**: implicit merging only recognizes fixed file names, so renaming a file silently stops the merge. The package then ships without a core, the bundler still succeeds, and the failure only surfaces on the user's machine as `resolve_core_binary → Err`. With an explicit `--config`, the same rename produces a hard `failed to read configuration file`.
  (The macOS file was originally named `tauri.macos.conf.json`, which would be merged implicitly even though it is arm64-specific — meaning a bare `tauri build` on an Intel Mac would bundle the arm64 core. It has been renamed to `tauri.macos-arm64.conf.json` to remove that implicit default.)

These invariants are enforced mechanically by `node scripts/verify-packaging.mjs confs` (run in CI before every packaging job, reproducible locally). After the build, the `payload` and `assets` modes assert that the artifact contains exactly one core for its own platform and that the artifact name satisfies the updater's package-selection contract.

All of the above ask whether what **should** be there is there. The opposite direction is guarded by a fourth mode, `inventory` — a **package content allow-list**: it enumerates every file in the resource payload tree and reconciles it against an explicitly registered allow-list; **one extra file turns it red**, and every entry must state why it belongs in the package (see `payloadAllowRules()` in `verify-packaging.mjs`).

```bash
node scripts/verify-packaging.mjs inventory --label linux --static                       # no artifact needed: derived from conf x working tree
node scripts/verify-packaging.mjs inventory --label linux --root target/release/bundle   # artifact scope
```

It was added (2026-08-29) after a batch of stowaways that had **already shipped**: `resources/data/README*.md` (developer docs, now moved to `docs/geo-rulesets*.md`), zero-byte `.gitkeep` placeholders, and the dashboard's gh-pages/PWA leftovers (`.nojekyll`, `sw.js`, `registerSW.js`, `workbox-*.js`, `manifest.webmanifest`, now stripped right after extraction in `scripts/fetch-dashboard.mjs`) — none of which turned anything red at the time. The **invariant E** added to `confs` in the same batch closes the config-side entrance of the same class: adding `"../ui/src/"` to all four per-platform confs used to leave `confs` at rc=0, shipping the entire `ui/src` tree inside all four installers.

`inventory` states its reach honestly: it counts what the bundler lays down from `bundle.resources`, not the ELF binaries and host shared libraries that the bundler / linuxdeploy place themselves (those are not controlled by `bundle.resources`); the number of out-of-reach files is printed. On Windows, where NSIS leaves no bundle-side copy, it degrades to a cargo staging count and says so in its output ("staging check, not artifact verification").

## Continuous integration

Six workflows divide the work (`.github/workflows/`):

- **`ci.yml`** — the Rust gate; its job is "is the change correct". Three parallel jobs:
  - `fmt + clippy (<os>)`: formatting and clippy (`-D warnings`); the Linux leg also runs rustdoc and resolves the Cronet sources.
  - `cross-target clippy + android face` (Linux only): clippy for the four cross targets (Windows / macOS / Android / iOS), a check that the vendored third-party copies under `vendor/` produce no diagnostics on those targets, and checks of the Android impact registry and the cross-target exemption table. Packages with C dependencies are exempt on some targets (see `scripts/cross-target-exempt.json`), so this job does not replace the native legs.
  - `build + test (<os>)`: `cargo build` and `cargo test`.

  Triggers and platforms:
  - A push to `main` runs only the Linux native tests plus the cross-target clippy (a documentation-only push does not trigger it).
  - The Windows and macOS native lint and tests run on pull requests, on manual dispatch, and for releases (a release calls this workflow from `package.yml` and is not published unless all three platforms pass).
  - A change that touches platform-specific branches must have the three platforms dispatched manually before it lands: `gh workflow run ci.yml --ref <branch>`; add `-f os=windows-2022` or `-f os=macos-14` to confirm a single platform. A direct push to `main` does not run those two platforms, so a failure on a native leg stays unseen until the next manual dispatch or release.

  The local mirror is `scripts/gate-rust.sh` (the cross-target gates need `--with-cross`); `scripts/gate-rust-ci-parity.test.mjs` keeps it in step with `ci.yml`. Setting `fmt + clippy (<os>)` and `build + test (<os>)` for the three platforms, plus `cross-target clippy + android face`, as required checks is recommended.
- **`ui.yml`** — frontend gate: `pnpm run build` (tsc + vite build), vitest, and Playwright; triggered on PRs and pushes to main.
- **`release-risk.yml`** — release-risk gate: triggered on PRs, the merge queue, and pushes to main; it classifies the changed paths inside a job and calls `desktop-core.yml`, `package.yml` (without uploading artifacts), and `android.yml` only for the affected faces.
- **`desktop-core.yml`** — call-only: four native runners verify the pinned release inputs, or build pinned source in source mode, and assemble the candidate bundle.
- **`package.yml`** — release engineering: four legs (Linux / Windows / macOS arm64 / macOS x64) running fetch + `tauri build` + artifact verification. Triggered by tags (`v*`), manually, or by a call from `release-risk.yml`. Its job is "can we produce a distributable installer". On a tag release it creates the draft release, uploads the desktop assets, then calls `android.yml` to upload the signed APK, and publishes after reconciling the full asset set.
- **`android.yml`** — call-only or manual: builds a release-profile APK and runs artifact-level checks; when called for a release it produces the signed APK.

## Windows installer and WebView2

Tauri 2 depends on the WebView2 Runtime. Windows ships a single **`Polaris_<version>_x64-win-setup.exe`** using `downloadBootstrapper` from `tauri.conf.json`: ordinary Windows 10/11 usually has it preinstalled, and when it is missing the installer fetches Microsoft's Runtime online. Polaris neither embeds nor mirrors the WebView2 Runtime, and does not maintain a second Windows installer.

Users on stripped-down / LTSC images or portable setups that lack the Runtime need to install [Microsoft Edge WebView2 Runtime](https://developer.microsoft.com/microsoft-edge/webview2/) from Microsoft first. A fully offline device could not download Polaris or fetch a subscription either, so the release pipeline no longer maintains a second installer and its verification workflow for that case.

The naming is not arbitrary. On Windows the updater (`crates/updater/src/github.rs`) splits package selection into **two disjoint rules by runtime form**:

| Runtime form | Selection criteria | Match |
|---|---|---|
| Installed (via NSIS) | `Polaris_` prefix + `_x64-win-setup.exe` suffix | `Polaris_<version>_x64-win-setup.exe` |
| Portable (run from an extracted zip) | `Polaris_` prefix + `_x64-win-Portable.zip` suffix | `Polaris_<version>_x64-win-Portable.zip` |

Hence the installer carries `win` explicitly and the portable build is a zip, placing them in disjoint namespaces so each rule is unambiguous. The "exactly one" guarantees are enforced mechanically in CI by `verify-packaging.mjs assets`.

**How the portable form is detected**: the portable zip contains a `portable.marker` file next to `polaris.exe`, which the app reads when checking for updates (`commands/updater::is_portable_layout`). It deliberately does not use environment variables such as `PORTABLE_EXECUTABLE_DIR` — those are injected specifically by electron-builder's portable target (a self-extracting stub), whereas this repo's portable build is a plain zip made with `Compress-Archive` and has no stub, so that variable never exists. After building the zip, `package.yml` opens it back up and verifies the marker really is inside; if it is missing, the whole leg fails hard.

⚠️ **Portable updates are "download, then replace by hand", not fully automatic**: portable users receive a zip and there is no installer. After clicking "Restart and install" on the update card, the app first shows instructions (the locations of the update archive and of the program folder); the connection is untouched at this point. Only when the user then clicks "Quit and open folders" does the app run the following steps in order (the portable leg of `update_install`):

1. Disconnect and stop the core (the same preparation gate as a normal quit; if the core cannot be confirmed stopped, the app keeps running and asks the user to retry).
2. Open the update archive and the program folder with the system handler (if either fails to open, the app does not quit).
3. Quit the app.

The user then extracts everything in the archive into the program folder, choosing to replace existing files, and starts the app again. The core `sing-box.exe` runs directly from `resources\win\` inside the portable folder and Windows does not allow a running image file to be overwritten, so stopping the core and quitting must come before the files are replaced — the app enforces that order instead of relying on the user to quit first. Automatic extract-and-replace has not been implemented: it would need a script that runs on its own after the main program exits, swaps entries in one by one and can roll back, and the folder it would replace is one the user placed themselves, with permissions and file locks the app cannot predict.

⚠️ **What the app cannot cover**: if a user bypasses the update card and extracts the archive into the program folder while the app is still running, files in use are skipped and the folder ends up with a mix of old and new files. The portable build does not go through the installer, so the installer step that first stops cores running from the installation folder does not exist here.

⚠️ A portable run that is handed the installer (or an installed run that is handed the portable archive) is treated as a form mismatch: nothing is installed, the file is handed to the system and the update card reports an error, so a second copy is never installed behind the user's back.

⚠️ If a user deletes `portable.marker`, the app goes back to treating itself as an installed build and starts offering the installer instead. **No automated mechanism can guard against this**, so the file itself states that it must not be deleted.

## Bundled core and privileged helper

Desktop production packages use the selected polaris-box core for this app version. Linux and Windows include Cronet beside the core; macOS uses static inputs. Packaging freezes the actual helper bytes (and Windows cleaner), generates `core-manifest.json` as an App compile input, and verifies the final payload against it. Production builds accept neither uploaded cores nor environment overrides for the core path.

A change to the selected content identity requires an administrator installation transaction before TUN startup. The helper stores immutable versions by CoreID; runtime requests send the ID without uploading files. A newer compatible helper may be retained. Adding another core version does not stop another user's active core. If an old helper retains custody or an empty slot cannot be established, replacement is deferred.

Retention keeps the three most recently installed content identities and every version with a live native lease. Flat cores and old user caches are retired only after complete idle observation; unknown use preserves them. Public release asset cleanup follows verified App consumption and migration of active references.

Self-update preserves the published nested helper/core bytes. macOS verifies the staged and final bundle without deep resigning it. macOS and AppImage updates stage the candidate beside the destination, serialize replacement, and preserve the original files when replacement fails; a failed recovery retains its exact transaction directory. Downloaded installers remain available on failure. These source paths still require native carrier validation.
