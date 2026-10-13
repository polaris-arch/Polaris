# Formal mobile source and binding input

This is the Polaris App producer input, separate from the completed box CLI and
OpenWrt producers. The source candidate is the actual common immutable
`577d4b9a089b71d2f0a4ca334b87264b55c10478`, tree
`072dacb999c07c7948816f10efaaaf371da45eb9`, annotated tag
`v1.15.0-alpha.12` / object `5cedbe7a54594756da544af90ebfd1f403f2e091`.
The tag's CLI/OpenWrt publication does not approve a mobile output. No existing
rule requires a second M tag. The mobile role is explicit in this input and its
future producer receipt; it is not retroactively added to the raw tag annotation.

The formal tool choice is Go **1.25.5**, gomobile and gobind **v0.1.13** for both
mobile platforms. That matches the actual source go.mod, Makefile and generated
binding API. The historical v0.1.12 Apple policy/receipts retain their identity.
Diagnostic v0.1.13 results also remain diagnostic; they are not formal output
pins. No global Go/gomobile/NDK installation or upgrade is performed here.

The source input uses no App overlay or historical patch replay. Verification
checks the actual tag object, peel, commit, tree, go.mod/go.sum, source-owned tag
declaration and clean compiler-input inventory. `fork_input.py` does not require
the old nonempty patch list or a CLI source receipt.

Formal binding plans are explicit:

- Android: one API24 AAR, `arm64-v8a, armeabi-v7a, x86, x86_64`, 16KiB PT_LOAD
  requirement, Java17 and pinned r30-beta3 NDK. The producer uses a private SDK
  view containing only that NDK; ambient installed NDK28 cannot win a fallback
  lookup. It does not invoke the helper that also builds API21/no-Naive legacy.
- Apple: only `ios/arm64, iossimulator/arm64, iossimulator/amd64`, static
  XCFramework, library deployment minimum15 and App minimum17. No default
  macOS/tvOS targets. Actual selected Xcode/SDK/compiler identities must be
  collected before execution and checked by the eventual producer.
- Tags are copied from the actual source-owned libbox declaration, including
  gvisor, Naive, Tailscale and Apple-only DHCP/low-memory flags. The Go module
  remains `github.com/sagernet/sing-box`; only outer assets use `polaris-box-*`.
- Version is passed through fixed ldflags. `ReadTag`/`git describe`, an upstream
  cache tag, or a reused CLI BuildID cannot provide formal mobile identity.

`plan.py` verifies the input and prints a bind plan. It never executes a build,
copies an SDK into an App, installs tools, creates a tag or publishes assets.
Plan fingerprints bind source and target declarations; actual tool identities
must be added by the producer before compiling. `android_sdk.py` prepares the
scoped SDK view only under a fixed private work slot held by the caller's
exclusive producer lease. Tests are authored in `test_inputs.py` and unexecuted.

The explicit planning entry is `bash scripts/plan-mobile-core.sh android` or
`apple`, followed by `--source`, `--gomobile`, `--output`, and Android's
`--bootclasspath`. Supply the existing exact source checkout and isolated tool
paths. Planning does not resolve or download an SDK. Existing legacy build entry
bytes are kept intact so old frozen receipt/script identities remain resolvable.

The formal producer entry is now `bash scripts/build-mobile-core.sh android`
or `apple`, with required `--source`, `--go`, `--gomobile`, `--gobind`, `--work`
and `--cache`. Android also requires `--sdk --ndk --jdk`; Apple requires
`--developer`. It verifies the supplied exact source and uses one fixed `work/build-view`
shared-object Git clone at that exact commit. The generator writes/deletes its
own `source/build/<arch>` paths there; the supplied checkout stays untouched.
This build view shares the work/cache filesystem budget and inherited lease,
so separate producers cannot mutate one another's generator paths. It uses
scoped Go/module/GOPATH caches and directly binds `staging/Libbox.xcframework`
on Apple, preserving the generator's actual `Libbox` framework/module name.
Android selects the verified Java17 `java` and `javac` paths and records both
binary hashes/versions, regardless of an ambient Java21 PATH. The
pinned generator's marker directory is created inside that GOPATH: `gomobile
init` is never invoked because v0.1.13 would install gobind@latest.

`artifacts.py` is the shared fresh/cache predicate: four exact Android ABIs,
API24 ident notes, 16KiB PT_LOAD, actual dynamic runtime/JNI exports, generated
Java API, original Go BuildInfo/module/replacement/BuildID; Apple has exactly
three static architecture slices, bounded ar/Mach-O parsing, safe framework
internal links, generated Objective-C API and real deployment/platform facts.
`-x -work` preserves actual compiler objects and logs. `native_inputs.py` records
those byte identities plus original external static archives, and maps every
non-Go Apple archive member to actual compiler/runtime origins with multiplicity.
Cache hits reread original native inputs, original log and the complete output.
A changed producer commit can reuse identical producer code/tools while keeping
the original component receipt's candidate and BuildID unchanged.

`transaction.py` stages output/receipt/log together and journals exact fixed
paths and byte/tree pins. Failure or interrupted commit restores the original
pair and keeps rejected bytes and the original journal in bounded recovery
slots. Identical Android input materialization is a verified cache hit. Occupied
prior/rejected slots block replacement pending owner/reference review; nothing
recursively deletes an unknown or unique input to make a build proceed.

All local entries use `budget.py`'s atomic same-filesystem admission ledger and
child-inherited flock reservation, plus fixed work/cache locks. The common
ledger defaults to `~/.cache/polaris-build-budget`; an external volume needs one
explicit `POLARIS_BUILD_BUDGET_ROOT` shared by its concurrent callers. Each
mobile build reserves 16GiB of additional peak growth plus 1GiB system margin;
this conservative ledger subtracts all live reservations, including child-held
reservations after parent loss. Hosted jobs have private filesystems. One cache
slot per platform has a 20GiB limit. Success/failure/cancellation all record
actual inode-deduplicated slot bytes and filesystem free bytes. Original
component/native-input/log/recovery evidence remains retained for the first
unified review; cleanup is not asserted by a successful build. Existing unrelated
build allocators must join the same physical-filesystem reservation protocol
before concurrent local heavy validation; this entry cannot account for an
unregistered external builder.

Android's existing workflow uses `ci_android.py` to prepare pinned tools/source,
restore exact work/cache keys, unconditionally build or revalidate, and
materialize the verified input. Both unsigned and signed APK legs verify final
APK ABI/runtime bytes, R8 evidence and the same original AAR/receipt; signed
publication still goes through existing certificate/tag/draft gates. The formal
SDK has four ABIs; published App scope remains the existing two ARM ABIs. SDK,
original receipts/logs and final source-consumption receipts are retained as
Actions evidence, not new public SDK publication.

Apple's explicit uniform Mac entry is `python3 -B
scripts/mobile-core/build_apple_app.py` with the same source/tool/slot arguments.
It builds/revalidates the static SDK, freezes an App-specific link context, runs
the pure measurement-state Swift controls, and compiles/archives App and
PacketTunnel. `--simulator` selects a simulator link probe (`--simulator-arch`
arm64 or amd64); omit it for the unsigned device compile/link probe, or supply an
explicit `--signing-team` for that team's signing observation. Xcode's extension
pre-link hook verifies the exact link-context SHA and original static input.
`carriers.py` checks final Go carrier/module/BuildID, the actual original or
byte-equal DerivedData static archive and generated headers, linker-map member
origins, bundle/App Group/device-family/platform/minimum-OS facts, and absence of
installed static SDK payloads. It does not assert signing permission/trust,
installation, device lifecycle or App acceptance. No Apple CI is dispatched by
these source entries.
The source-owned `.github/workflows/apple.yml` now calls `ci_apple.py`
prepare/build/evidence from the existing Release Risk graph. Selection reads
actual changed paths through `apple-ci-impact.mjs`; the aggregate refuses a
selected failed/cancelled/skipped Apple leg. The hosted `xcode-27` runner must
actually provide Xcode27.0 build27A266a and both SDK27.0; admission fails without
fallback. Its one fixed `~/Code/polaris-mobile-apple-ci` root has one component
slot, one cache (20GiB maximum including a shared Rust target), and the three
finite device-arm64/sim-arm64/sim-amd64 App slots, built sequentially under the
existing 16GiB peak/1GiB margin reservation and cache lease. No signing secrets,
simulator boot, device deployment or mobile publication are part of that graph.
The hosted preview advertises 14GiB storage; actual free space must still pass
the unchanged 16GiB peak plus 1GiB margin admission. A smaller/unavailable runner
is an environment failure, never grounds to weaken the reservation or skip a
selected leg. The eventual unified run must establish actual runner resources.

The formal builder compiles through `apple_rust.py build` under its inherited
reservation before Xcode starts; the Xcode Rust phase runs `apple_rust.py verify`
with the admitted
link context, explicit Cargo target, locked dependencies and iOS config. It
checks the actual static archive/start_app and saves its byte/source/tool facts;
it does not require a running Tauri CLI options server or regenerate tracked
PBX/Cargo files. The original CLI development hook remains. The release wire
check consumes the exact verified Apple static slice rather than a desktop
binary. SDK origins, real Rust input, final carriers, link maps and original
logs are retained in one indexed evidence ZIP; entire caches/checkout/target
copies are excluded. Failed chains retain partial evidence without acceptance.


`scripts/libbox-patches` and the old Apple six-patch entry remain historical
alpha8 compatibility inputs. Their raw receipts are not relabeled; the small
`historical-ci-fragments.json` pins only retired caller fragments for existing
negative tests. New formal source controls live in `test_inputs.py` and
`test_outputs.py` and enter the existing host Node gate through
`mobile-source-unit.test.mjs`. This source batch has not executed those tests,
compiled native output, dispatched CI, signed/installed Apps or published mobile
assets. Contract artifact pins/producer receipt remain null, and
AppAccepted/devicePassed/publicationEligible remain false until actual results.
