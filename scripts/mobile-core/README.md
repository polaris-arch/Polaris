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

The NDK input distinguishes the unchanged numeric SDK-view directory
`ndkRevision=30.0.16138531` from the archive's exact
`ndkSourceRevision=30.0.16138531-beta3`. Admission checks the complete
`Pkg.Revision`, `Pkg.BaseRevision` and `Pkg.ReleaseName`; it strips property key
spacing and rejects duplicate keys, never drops the preview suffix. The new
required source-contract field enters the existing source/tool fingerprints and
cache key; older receipts/cache entries cannot be relabeled. The hosted entry
retains the original small properties bytes and their pinned observation even
if a later producer step fails. Historical manifests/archive pins are unchanged.


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
Before that input is frozen, exact remote module versions already covered by
the source `go.sum` are admitted into the same module cache if their directories
are missing. Go1.25.5's `go list -m -json` reports those directories according to
cache availability; a cold graph must not be frozen and then change merely
because the original bind populated the cache. Download admission uses one
fixed `module-cache-admission/admission.mod` plus `.sum` copy (each at most 1MiB,
at most 1024 exact arguments), never the frozen checkout's module files.
Returned module/version/h1 inventories and unchanged source bytes are checked,
then the full graph and `go mod verify` are read again. No graph identity or
location is discarded. This uses the existing lease, 20GiB cache and reserve;
retained admission inputs refuse instead of being overwritten or deleted.
Only the added download invocation uses a separate bounded collector: 1MiB
stdout, 4KiB discarded stderr (one overflow sentinel each), 8KiB lines,
32768 lines, 4KiB JSON strings, depth8/65536 structural tokens and at most the
requested 1024 objects. Duplicate keys, nonfinite constants, Error objects,
unknown fields or wrong record types refuse with fixed safe reasons. Collection
keeps the original 3600-second limit plus at most two seconds of owned-group
cleanup; a remaining group retains ledger ownership and rejects. No
`original-module-cache-admission.log` is created. The failure/success artifact
gets only `module-cache-admission-observation.json` counters/limits/reason and
cleanup status, never original JSON, stderr, URLs or error bodies. Its `complete`
means pipe/JSON collection only; full checksum/input predicates still follow.
Whole-process RSS is unmeasured. Other established producer/tool calls retain
their original behavior and evidence scope.
An actual unequal cached binding still refuses original generator output reuse.
Its derived `cache-binding-observation.json` records only fixed changed-field
labels, row/directory/sum counts and already validated fingerprints, never input
values or module URLs; the hosted failure evidence retains it. The old failed
attempt did not record its current binding, so its exact differing fields remain
unknown. Cache admission addresses the confirmed availability-dependent graph
mechanism; the next exact-SHA run must establish the actual reuse outcome.

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
actually provide Xcode27.1 build27A9269 and both SDK27.1, explicitly selected via
`/Applications/Xcode_27.1_beta.app`; admission fails without fallback. This SDK
declares the viewport's iOS27.1 APIs; their runtime availability guards and the
iOS17.0 App deployment target remain. Its one fixed `~/Code/polaris-mobile-apple-ci`
root has one component slot, one cache (20GiB maximum including a shared Rust
target), and the three
finite device-arm64/sim-arm64/sim-amd64 App slots, built sequentially under the
existing 16GiB peak/1GiB margin reservation and cache lease. No signing secrets,
simulator boot, device deployment or mobile publication are part of that graph.
The hosted preview advertises 14GiB storage; actual free space must still pass
the unchanged 16GiB peak plus 1GiB margin admission. A smaller/unavailable runner
is an environment failure, never grounds to weaken the reservation or skip a
selected leg. The eventual unified run must establish actual runner resources.

Hosted and local entries now share the same protected reservation ledger and
inheritable descriptor; hosted admission never substitutes a space-only yield.
The child's reservation identity includes the actual inode, filesystem, owner,
private mode and held flock. Compiler children receive that descriptor before
any Cargo invocation; the Xcode metadata-only verifier does not reuse it.

Both the direct builder and hosted entry materialize the actual iOS config's
28 `.srs` files and three license/notice texts into the generated
`gen/apple/assets/_up_` tree before the App link context is frozen. Unknown
stages or differing retained bytes are preserved and refused. The context binds
source bytes/modes; the final IPA predicate requires the same complete resource
inventory, bytes and modes in the real App bundle, alongside App/PacketTunnel
static-link/source checks. An empty assets folder cannot satisfy the predicate.

The formal builder compiles through `apple_rust.py build` under its inherited
reservation before Xcode starts; the Xcode Rust phase runs `apple_rust.py verify`
with the admitted
link context, explicit Cargo target, locked dependencies and iOS config. It
uses `cargo rustc --lib --crate-type staticlib` only for this iOS archive stage;
the shared Cargo manifest retains Android's cdylib and the other library types.
Swift bridge symbols must be native global definitions in this archive before
it is staged, then resolve in the subsequent full Xcode App link. The locked
Tauri 2.11.5 and dialog/notification/shell 2.7.2/2.3.3/2.3.5 Swift packages declare
their C entries with internal access. Their iOS inputs are staged under the
existing Cargo `OUT_DIR/polaris-c-exports` slot (256 entries/4MiB combined cap),
including the plugins' relative `.tauri/tauri-api` dependency. Only the seven
known `@_cdecl` declarations receive `public`; versions, unique declarations,
ordinary paths and byte budgets are checked, while implementations and package
manifests/locks are preserved. Registry sources are untouched, and macOS keeps
its original build path. The source-owned Polaris entry is explicitly public.
The Rust receipt binds all eight observed global definitions and the three
export-policy source hashes; Xcode's verification phase checks that binding.
The subsequent carrier predicate also parses the device outputs' weak system
Swift dylib loads and three observed executable rpaths, retaining exact command
payloads and version fields. These commands are limited to linked executables;
string bounds/UTF-8/zero padding, system Swift paths, rpath inventory/duplicates
and version relationships fail closed. Unknown commands still fail. Parsing a
command does not accept a carrier, signing or device execution; the full
App/extension/source/static-link predicates remain required.
The pure Foundation control executable uses the selected Xcode's explicit Mac
SDK and the actual host architecture with the existing Mac deployment target.
Its environment is a separate copy; the iOS SDK/target used by the subsequent
full App/PacketTunnel link is unchanged. The control is compiled and run, never
skipped to make a selected Apple gate green.
It checks the actual static archive/start_app and saves its byte/source/tool facts;
the native symbol check uses the existing `nm --no-llvm-bc -gU` mode, requires
success plus a defined `_start_app` and all eight global Swift bridge text
symbols, and retains its full output. Local/private names cannot satisfy it.
Rust's compiler-builtins IR is left to the unchanged final Xcode link; this
check does not claim IR compatibility or final carrier acceptance. Original
archives are retained even when no accepted Rust receipt has been written.
The plugin stages its unchanged `Package.swift`/`Sources` and source-owned
`Package.resolved` into the
Cargo-owned `OUT_DIR/polaris-ios-package/ios` (256 entries/4MiB input cap).
Tauri's existing builder receives that absolute path; its local `.tauri` bridge
and SwiftPM's `Package.resolved` are generated in the same build slot. Source
changes invalidate Cargo through the original manifest/lock/source paths.
Restaging removes stale owned Swift sources and restores the frozen resolver
lock; after the original builder returns, its actual complete resolver pin must
match that lock. Known SwiftPM v1/v2/v3 envelopes are checked explicitly,
including v3 origin-hash shape; unknown schemas, extra fields/pins and changed
repository/revision/version are rejected (formatting changes are allowed).
This isolates generated files without ignoring them in the checkout or changing
SwiftPM's upstream Tauri dependency range. `Cargo.lock` does not freeze that
Swift dependency resolution. The new lock pins the verified public SwiftRs
1.0.7 tag commit, matching the Rust bridge's existing version; it is a new
source input, not a reconstruction of the old 323-byte observed lock, whose
dependency pins were not retained. Missing, malformed or changed resolver pins
fail the build without exporting their body.
After formal environment/slot admission, the Rust entry observes source status
before link verification and after archive copying. Earlier admission failures
need not have an observation. Capturing status is capped at 16KiB stdout, 1KiB
stderr, 64 paths of at most 512 bytes, 1MiB total file hashing and a 10-second
collection deadline, with up to two seconds to terminate the owned child.
One sentinel byte beyond each pipe/hash cap detects overflow; capture buffers and
metadata cardinality are bounded, while whole-process RSS remains unmeasured.
The derived JSON is capped at 64KiB; unknown paths have opaque hashes, and only
reviewed exact names are displayed. No stdout/stderr, diff or file bodies are
exported. Overflow or collection failure records incomplete observation and
rejects; every dirty checkout still rejects. These are metadata observations,
not original source snapshots, root-cause confirmation or App acceptance.
It does not require a running Tauri CLI options server or regenerate tracked
PBX/Cargo files. The original CLI development hook remains. The release wire
check consumes a receipt-bound thin archive selected from the fully inspected
original container by the actual Cargo CPU and iOS/device or simulator platform.
The original bind log records a single-input device `lipo -create` and a
two-input simulator merge; the failed run retained no staging binary, so its
actual fat count remains unobserved. The parser admits the format's one-entry
wrapper only under the exact device inventory, and validates each header against
every code member, while the formal consumer requires the exact Info.plist
device arm64 and simulator arm64+x86_64 inventories and all three final targets.
For the fat simulator SDK it independently selects arm64 or x86_64, retains the
original archive pin plus exact offset/length and thin bytes, and rechecks them
before Xcode link. The Rust release guard validates every code member's Cargo
CPU/subtype and platform before scanning that selected archive's descriptor;
a whole fat input is rejected. Original component/producer bytes are unchanged. SDK origins, real Rust input, final carriers, link maps and original
logs are retained in one indexed evidence ZIP; entire caches/checkout/target
copies are excluded. Failed chains retain partial evidence without acceptance,
including actual unaccepted staging frameworks when inspection failed before
a final component/receipt was installed. Parser failures log the actual binary
path, first header bytes, fat count and declared architectures; those observations
are distinct from inferred container properties based on bind commands.


Apple native-input schema v2 stores each complete source-candidate list once
in `originsByDigest`; each static member keeps its ordinal and digest reference.
Equal bytes from distinct source paths/archives/ordinals remain distinct rows.
Producer and consumer reconstruct the table from original pinned files and
check exact references and per-target multiplicities. Legacy inline v1 inputs
are verified without rewriting their original receipt. The observed 57b3
152757078-byte envelope was dominated by repeated origin lists; it is not a
required capacity. Component/link envelopes return to 64MiB, controls/Android
remain 16MiB. Fingerprints and pretty serialization stream bounded tokens;
shape/lexical checks cap depth at 64, nodes at 500000 and strings at 1MiB.
The producer logs exact new serialized bytes and canonical value bytes per
top-level field. These are metadata bounds, not a claimed measured process RSS
or native/carrier acceptance; native archive parsing remains separate.

`scripts/libbox-patches` and the old Apple six-patch entry remain historical
alpha8 compatibility inputs. Their raw receipts are not relabeled; the small
`historical-ci-fragments.json` pins only retired caller fragments for existing
negative tests. New formal source controls live in `test_inputs.py` and
`test_outputs.py` and enter the existing host Node gate through
`mobile-source-unit.test.mjs`. This source batch has not executed those tests,
compiled native output, dispatched CI, signed/installed Apps or published mobile
assets. Contract artifact pins/producer receipt remain null, and
AppAccepted/devicePassed/publicationEligible remain false until actual results.
