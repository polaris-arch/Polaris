# Formal mobile source and binding input

## Selected alpha.12.1 source input

The selected Polaris App mobile producer input is
`814da362838e96e54f904836a92c2b0d8c3ae29b`, tree
`ad574f93026557b49edecb492bd7ecdb851057c1`, annotated tag
`v1.15.0-polaris-alpha.12.1` / object `8ecda357ea755a1295336a35ec7eaf1fe2daebc2`.
The tag object peels directly to this reviewed commit. The complete published
source tree and its bytes/modes, module files and source-owned libbox declaration
were checked; a tag alone is not an AAR/XCFramework or App acceptance receipt.

The source contract uses `polaris-reviewed-kernel-source-v2` for the exact nine
local vendor replacements (2838 files), including the 45-file tfo-go tree
`97b124004140efa84aeb0f57d9bd41d238f0c9b5`. Actual module versions, replacement
paths, Git subtree identities and file counts must agree. The source go.mod hash
is `2c41a5a45e22341d2f8ca750134edeb5e09cf79d3366575a5a0966cc10a6162c`;
go.sum, the libbox tag declaration and the 355-file Meta replacement tree and
binding/patch hashes retain their independently checked identities.

This source includes RuntimeCapabilities, retryable runtime persistence and the
RuntimePersistenceHandler API consumed by the existing App. Generated Java/JNI
and Objective-C declarations, actual Kotlin/Swift compilation, all four Android
ABI ELF checks and both Apple slices still require this input's own producer and
consumer evidence. Source API presence and prior diagnostic outputs do not
substitute for those checks.

Input declarations never issue artifact acceptance: `artifactPins` and
`producerReceipt` stay null, and `AppAccepted`, `devicePassed` and
`publicationEligible` stay false even after a build. Real evidence belongs in
the separate original component, consumption and carrier receipts. No formal
public mobile download URL is invented. Raw kernel All libraries do not replace
App-built component receipts.

Mobile source admission is independent of desktop public asset availability.
The old desktop `nextDelivery` and release contracts retain their a4d identity.
`fixtures/selected-source-contract-a4d.json` preserves the exact old mobile input
bytes (SHA-256 `d2defda63a5b8609edf8d4e86c5dc9c6c49bd9ad3dbfe437bdc863b6bd2345c0`)
for those historical desktop assertions; it is not the selected producer input
or a receipt. Old a4d/c002/fe182 sources and diagnostic results are not relabeled.

The formal tools remain Go **1.27.2**, with source minimum Go **1.27.0**, and
gomobile/gobind **v0.1.13** on both platforms. This remains the original
`polaris-mobile-fork-input-v1` input schema. Consumption requires its own
`polaris-mobile-component-v1` receipt, exact sourceContract/nativeInputs,
generated API and ABI checks, and the `polaris-mobile-<input digest>` BuildID.
A different source input changes that digest and DirectoryRetirementSourceIdentity;
old component/cache receipts cannot be reused under the new identity.

The source input uses zero App overlay and no historical patch replay.
Verification checks the actual annotated tag object, peel, commit, tree,
go.mod/go.sum, source-owned tag declaration and clean compiler-input inventory.
The existing App producer binds DirectoryRetirementSourceIdentity to the exact
selected sourceCommit; a CLI BuildID or source receipt cannot supply that identity.

Formal binding plans are explicit:

- Android: one API24 AAR, `arm64-v8a, armeabi-v7a, x86, x86_64`, 16KiB PT_LOAD
  requirement, Java17 and pinned stable r30 NDK. The producer uses a private SDK
  view containing only that NDK; ambient installed NDK28 cannot win a fallback
  lookup. It does not invoke the helper that also builds API21/no-Naive legacy.
- Apple: exactly `ios/arm64, iossimulator/arm64`, one static XCFramework
  with exactly two libraries: iOS device (no variant) and iOS simulator
  (`simulator` variant), each containing only arm64. The observation slice keys
  are `ios-arm64` and `ios-arm64-simulator`; each `LibraryIdentifier` must be a
  safe basename, without requiring those exact directory names. The finite App
  slots are `device-arm64` and `sim-arm64`.
  Library deployment minimum15 and App minimum17 remain distinct. Intel
  simulator, macOS and tvOS are outside the formal target set. Xcode **27.1**,
  build **27A9275**, and both iOS/Simulator SDKs **27.1** are mandatory; actual
  selected compiler/SDK identities are checked without version fallback.
- Tags are copied from the actual source-owned libbox declaration, including
  gvisor, Naive, Tailscale and Apple-only DHCP/low-memory flags. The Go module
  remains `github.com/sagernet/sing-box`; the pinned source repository is
  `polaris-arch/sing-box-polaris` at `v1.15.0-polaris-alpha.12.1`. Published core
  assets use `sing-box-polaris-*`; formal mobile output remains Libbox.
- Version is passed through fixed ldflags. `ReadTag`/`git describe`, an upstream
  cache tag, or a reused CLI BuildID cannot provide formal mobile identity.

The formal Android NDK is stable r30, with both `ndkRevision` and
`ndkSourceRevision` exactly `30.0.16248370`. The explicit
`ndk-r30-linux.json` profile binds the official Linux archive size, SHA1/SHA256,
root and exact 107-byte source.properties hash. Admission also checks the full
`Pkg.Revision`, `Pkg.BaseRevision` and `Pkg.ReleaseName`, rejecting duplicate
keys and mixed preview/stable declarations. This profile and the installer enter
the producer source inventory, tool/cache fingerprints and receipt identity.
Older receipts/cache entries cannot be relabeled as this input.

Both formal hosted SDK paths check the already-accepted standard Android SDK
license read-only before SDK installation, with stdin closed for package installs.
The NDK installer repeats that check before any download or extraction. Missing
acceptance stops the build; no license is accepted or written automatically.
The explicitly selected installer uses a separate private versioned stable slot
and refuses retained slots without deleting them. Legacy no-flags installer
behavior, beta manifests, archive pins and historical fixtures stay unchanged.
The hosted entry retains source.properties and its pinned observation even if a
later producer step fails. Rust keeps its separate NDK 27.3.13750724.

The earlier stable-r30 migration was toolchain-only on a4d085/alpha.12; its
source and receipts remain historical. This alpha.12.1 source migration does not
change that stable profile or claim new-App compatibility, four-ABI production
results, APK/device acceptance or publication eligibility before actual checks.


`plan.py` verifies the input and prints a bind plan. It never executes a build,
copies an SDK into an App, installs tools, creates a tag or publishes assets.
Plan fingerprints bind source and target declarations; actual tool identities
must be added by the producer before compiling. `android_sdk.py` prepares the
scoped SDK view only under a fixed private work slot held by the caller's
exclusive producer lease. The pure source suites exercise admission without running a native producer;
those tests are distinct from actual component and App acceptance.

The explicit planning entry is `bash scripts/plan-mobile-core.sh android` or
`apple`, followed by `--source`, `--gomobile`, `--output`, and Android's
`--bootclasspath`. Supply the existing exact source checkout and isolated tool
paths. Planning does not resolve or download an SDK. The legacy Apple
`ios-libbox.py build` entry, including `--historical` and `--component --phase
assemble/publish`, is retired and refuses before reading source, policy or tools
or creating output. Its `build-libbox-ios.sh` wrapper reaches the same refusal.
The existing historical `verify` / `check-inputs` and explicit C2/C3 diagnostic
entries remain available within their original evidence scopes. Historical
policies and raw receipts are never rewritten or relabeled; the verifier keeps
the old build-only pins and the fixed pre-retirement b66 historical-only
producer pin, rather than treating the retired driver as a new producer.

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
two arm64 platform slices, bounded ar/Mach-O parsing, safe framework
internal links, generated Objective-C API and real deployment/platform facts.
`-x -work` preserves actual compiler objects and logs. `native_inputs.py` records
those byte identities plus original external static archives, and maps every
non-Go Apple archive member to actual compiler/runtime origins with multiplicity.
Cache hits reread original native inputs, original log and the complete output.
A changed producer commit can reuse identical producer code/tools while keeping
the original component receipt's candidate and BuildID unchanged.
Apple cache identity includes the exact arm64-only target set in the source/tool
fingerprints and cache namespace. A historical three-target cache is not a
fallback or migration source for this contract. Its receipt and native-input
bytes remain original; formal consumption requires the exact current two-target
inventory, including rejection of an extra Intel simulator entry or member.
Device arm64 and simulator arm64 are separate platform identities even though
the CPU is the same.
Hosted Apple complete-component reuse is an optional optimization for ordinary
Release Risk main pushes only. `component-cache-key` recomputes source, module,
actual tool/SDK, producer-code and effective native compilation inputs. The
exact `apple-go-component-v1` Actions key has no prefix fallback. PR/manual/tag
executions neither restore nor save this package. Generator tools are installed
with `-trimpath`; only admitted module-root locations and the generator filename
header are normalized for comparison. Original receipt paths and BuildIDs are
never rewritten. Native compilation overrides remain identity inputs; custom or
unrecognized effective compiler/CGO settings and ambient include/library/config
paths disable both restore and export, leaving the normal fresh build available. The
formal Apple execution environment excludes host PKG_CONFIG_PATH search
extensions (including setup-python's injected directory) before both production
and verification; this is an actual input boundary, not a cache-path exception.
Android and other native overrides retain their existing behavior.

The package contains the original receipt, two-slice output, bind log, compiler
objects, external archives, generated-source and local-replacement closure. It
is validated before restoring a previously unused historical private-root
namespace, then all original native/API/output predicates run again. A current
consumer observation identifies `trusted-main-cache`, retains the original
producer, and explicitly does not claim historical provider authentication or
current-run component production. Current evidence retains the complete package,
so old Actions runs/artifacts are not required for later validation.

The trust boundary is all repository executions allowed to write main-scoped
Actions caches, as with compiled Rust dependency caches; self-reported hashes
are integrity checks, not independent producer authentication. Missing, invalid
or incompatible restored packages rebuild. Current source/tool admission,
filesystem failures and failures after original installation are fatal. An
occupied/corrupt immutable cache entry is not overwritten or promoted; until it
is evicted, consumers may continue rebuilding safely. Export occurs only after
complete local verification and only for a newly produced component.

Rust download and compiled-dependency keys are independent. Download identity
retains exact Cargo format/tool bytes and lock/manifests/source configuration.
Compiled identity conservatively excludes only `android_sdk.py` and the two NDK
installer/archive files; shared `ci_android.py` and `android-source.py`, unknown
producer files, Apple workflow, tools, SDK/configuration and flags remain
relevant. Full producer provenance and final App-product exclusion are unchanged.
New key versions initially miss. These cache changes do not measure or promise
a speedup for a genuinely cold build or the final App crate.

Before that input is frozen, exact remote module versions already covered by
the source `go.sum` are admitted into the same module cache if their directories
are missing. The pinned Go1.27.2's `go list -m -json` reports those directories according to
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
PacketTunnel. `--simulator` selects the arm64 simulator link probe
(`--simulator-arch arm64` only); omit it for the unsigned device compile/link
probe, or supply an explicit `--signing-team` for that team's signing observation. Xcode's extension
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
actually provide Xcode27.1 build27A9275 and both SDK27.1, explicitly selected via
`/Applications/Xcode_27.1.app` (the installed 27.1 release candidate); admission
fails without fallback on any other build or SDK. This SDK
declares the viewport's iOS27.1 APIs; their runtime availability guards and the
iOS17.0 App deployment target remain. Its one fixed `~/Code/polaris-mobile-apple-ci`
root has one component slot, one cache (20GiB maximum including a shared Rust
target). The default App scope is device-arm64; `include_simulator: true`
(or Release Risk manual `apple_include_simulator: true`) additionally selects
sim-arm64, sequentially under the
existing 16GiB peak/1GiB margin reservation and cache lease. No signing secrets,
simulator boot, device deployment or mobile publication are part of that graph.
The hosted preview advertises 14GiB storage; actual free space must still pass
the unchanged 16GiB peak plus 1GiB margin admission. A smaller/unavailable runner
is an environment failure, never grounds to weaken the reservation or skip a
selected leg. The eventual unified run must establish actual runner resources.

The shared component and all requested carriers run in the same admitted hosted job.
The public Libbox component still contains both device and simulator ARM64
slices. Component two-slice validation does not claim simulator App validation.
Default success proves only the unsigned device-arm64 App + PacketTunnel
compile/link checks; it requires no connected device and proves neither signing
trust nor execution on an iPhone. Simulator App compilation is manual opt-in.
The official selected RC rollout must finish before dispatch; early admission
does not pin a rolling hosted image. The local-import adapter reuses the frozen
component and the existing complete original-source/compiler/SDK byte verifier,
without extracting over files or manufacturing a new receipt. Each carrier has
its own work/derived-data and target triple; execution is initially sequential
because the checkout's arm64 Externals staging is shared. This reduces cross-host
drift, not a claim of faster wall time.

Each slot exports create-new summary files under summaries/<slot>. Its original
linked Rust archive is copied and checked before another arm64 platform can
replace staging; success and failure originals have distinct slot ZIP names.
A failed slot does not suppress the remaining selected slots, and the final
aggregate independently receives the declared scope and requires exactly its
`requiredSlots`, including actual final App/PacketTunnel verification for every
requested platform. Missing, extra, skipped or failed required slots cannot pass.
The upstream host job must succeed, including original and summary uploads.
New component/carrier handoff wrappers use v2 with fingerprint-bound
`carrierScope`/`requiredSlots`; new unsigned-chain v3 reports `validatedSlots`,
`completeSlots`, and `notRequestedSlots`. A default sim-arm64 slot is not
requested, never passed. Historical v1 wrappers and unsigned-chain v2 retain
strict two-carrier semantics and cannot be relabelled device-only. Original
component/App/Rust receipts are retained byte-for-byte.
Shared component uploads, protobuf setup, cache setup and local verification
must all succeed before any carrier is selected; a carrier failure does not
change that shared readiness for later slots. Slot ZIPs retain the independent
stdout/stderr raw streams even when the combined log is unfinished. Staging
bytes that disagree with a Rust receipt are copied as explicitly unaccepted
evidence with their actual hash. Unsafe or incomplete preservation blocks the
next slot sharing that staging path, while different staging remains eligible.
After an original ZIP uploads, retirement borrows the same work/cache locks,
checks the retained archive's native bytes, then removes only that slot's
rebuildable Rust target triple and derived-data. A surviving child-held lock or
incomplete original preservation prevents retirement. Component inputs, shared
host dependencies, receipts, logs, IPA and original ZIPs remain, with the fixed
20GiB cache bound and existing build reservation unchanged.

Rust registry and dependency caches are reported separately. Each target/profile
uses an explicit restore/save path through a stable ignored cache staging
directory, rather than archiving a run-id-specific absolute Cargo target path.
Dependency directories move on the same filesystem without a second snapshot;
save follows successful carrier/original/summary upload and precedes retirement.
Cache keys bind scope, target, profile, actual Rust toolchain, selected Xcode/SDK
and source-owned build inputs. Cache hits never skip Cargo, link, or original
receipt verification and never substitute an accepted App product. Cache
compression also requires disk headroom; no warm-hit or speedup is claimed until
an actual subsequent run establishes it.

CPU/affinity/quota, load and memory observations guide child Go/Rust/Xcode
concurrency. The 2GiB/compiler plus 1GiB headroom is an unmeasured estimate,
not a memory reservation or compilation admission condition. Low or unknown
memory uses one task; Darwin free/inactive/speculative pages are a reclaimable
subset, not the complete OS available memory. The real OS observation and
cgroup limit/usage/headroom remain separate. Samples are flushed before
explicit child limits are validated, so invalid limits retain the observation.
An actual compiler/OOM failure remains a failure; this policy does not predict
peak RSS or relax the independent filesystem reservation.

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
The formal consumer requires exactly two Info.plist entries: device arm64 and
simulator arm64, each with one arm64 architecture. The bounded parser can retain
its structural handling of thin archives and supported one-entry fat wrappers;
that capability does not authorize a multi-architecture simulator component.
The target-specific extraction retains the original archive pin plus exact
offset/length and thin bytes, and rechecks them before Xcode link. The Rust
release guard validates every code member's Cargo CPU/subtype and platform
before scanning that selected archive's descriptor; a whole fat input is
rejected at that boundary. Sharing the arm64 CPU never makes device and
simulator archives interchangeable. Earlier two-input simulator merge logs and
three-target receipts remain historical evidence only, without new acceptance.
SDK origins, real Rust input, final carriers, link maps and original
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

The actual8795 ARM64 simulator App/PacketTunnel adds `LC_CODE_SIGNATURE`.
The carrier parser bounds its single nonempty MH_EXECUTE payload inside one
`__LINKEDIT` segment and disjoint from sections/other tables, retaining an
opaque payload SHA256 and `trustValidated=false`. It does not interpret the
CodeDirectory, calculate CDHash, validate a signer or assert signature trust.
These structural controls do not substitute for actual device-arm64 final
carrier verification, or sim-arm64 final verification when explicitly requested. Historical Intel simulator observations
are outside the current contract.

`scripts/libbox-patches` and the old Apple six-patch declarations remain
historical alpha8 compatibility inputs; their production build entries are retired.
Both `scripts/build-libbox.sh` and direct `scripts/libbox-patches/build.py`
refuse before source/tool/cache access or publication and point to
`bash scripts/build-mobile-core.sh android` with the required arguments above.
Historical read-only Android receipt predicates remain importable.
Their raw receipts are not relabeled; the small
`historical-ci-fragments.json` pins only retired caller fragments for existing
negative tests. New formal source controls live in `test_inputs.py` and
`test_outputs.py` and enter the existing host Node gate through
`mobile-source-unit.test.mjs`. This source-only gate does not compile native
output, sign/install Apps or publish mobile assets. Running it cannot establish
component or App acceptance. Input-contract artifact pins/producer receipt stay
null and AppAccepted/devicePassed/publicationEligible stay false; actual results
belong in separate original receipts rather than these input declarations.

## Exact-tree main merge evidence reuse

`apple_merge_reuse.py` is an opt-in read-only evidence path for the Release Risk
workflow's ordinary `push` to `refs/heads/main`. It admits only a same-repository
PR that GitHub confirms was merged at the current commit: either an exact-head
fast-forward (`current == PR head == merge_commit_sha`) or the original exact
two-parent merge (`parents == [PR base, PR head]`). A fast-forward's immediate
parent need not equal the old PR base. The current commit, PR head, and actual
executed reusable-workflow commit must have identical Git trees. Source
checkout, caller/reusable workflow files, source contract and
producer/consumer/verifier code inventories remain byte-bound. A push without
that merged-PR proof, a squash/rebase commit differing from the original head,
fork PR, arbitrary PR claim, tag, simulator request, manual dispatch, merge
group, incomplete listing or unavailable API selects a full build. Missing a
matching successful original PR execution contract also selects a full build;
this does not require creating a PR for other work. Human review and merge authorization remain in the existing workflow;
no GitHub APPROVED decision or new branch-protection setting is required or
asserted by a machine receipt. Existing authorized main writers and GitHub's
run/artifact integrity are the trust boundary: main write access already permits
changing this verifier.

The main run `39102634576` at `f076d3da055581cdf004d0d5147e712b44065a16`
selected the full native path. Its legitimate PR 15 fast-forward is incompatible
with the old two-parent-only verifier; this run is not an observed reuse success.
The corrected resolver also validates all workflow-output fields before writing,
including the existing `receipt_sha256` key that the old letters-only check
rejected. Original producer identities and evidence are never rewritten.

Every eligible Release Risk device-only PR still runs the complete native
pipeline. Four execution snapshots, before/after frontend+component and
before/after the carrier, bind actual Node/pnpm, Go/gomobile/gobind, Cargo/rustc,
Xcode/xcrun/Swift/clang and protoc identities, exact SDK versions/settings,
selected build environment overrides and source/workflow hashes. Before and
after observations must agree. Existing Rust receipts additionally bind the
locked release staticlib command, target, Cargo.lock, iOS configuration, actual
compiler binaries, link inputs and produced archive. The resulting
`apple-execution-contract-<run>-<attempt>` artifact is created only after the
same-run strict unsigned-chain aggregate and all six original artifact uploads
have succeeded and the effective Cargo inputs are supported. Its small sidecar binds their provider IDs/digests, exact
internal bytes/inventories and the original source/run/attempt. Standalone,
manual, main-fallback, merge-group and simulator full builds retain their normal
aggregate gate without this PR-only eligibility check.

On a later exact-tree merge, the current main verifier obtains the completed
successful original PR run and exact attempt from GitHub, verifies the real
workflow execution SHA and same-PR ref/branch, and requires both original Apple
jobs to have succeeded. It downloads all seven exact-attempt artifacts through
read-only provider APIs. Provider SHA-256 and size, bounded archive inventories,
internal file/receipt hashes, tool/SDK/source/input contracts, Rust archive,
linked archive and IPA bytes must match. Artifact names only select candidates;
cache hits, filenames, job skipping or successful statuses alone cannot admit
reuse. Only the Apple reusable caller and its resolver/sealer jobs receive the necessary
`actions: read`; workflow defaults and native component permissions remain
`contents: read`. No write scope, new credential or repository setting is added.
API routes are allowlisted to this repository and exact run/attempt/artifact IDs;
cross-repository identities are rejected. Storage redirects, receipts, logs and
artifacts receive no GitHub token. Archives are inspected,
never installed or executed, and their original absolute paths are treated as
data only.

The new `apple-merge-reuse-<run>-<attempt>` receipt refers to a single original
full producer and its artifact IDs/digests. It never edits old producer candidate,
run, attempt, path or receipt bytes; it never embeds an accumulating provenance
history or uses another reuse receipt as a producer. The final job validates
that exact same-run receipt and requires the native job to be skipped only in
this verified branch. A resolver miss/error or receipt-upload failure runs the
normal build. A later receipt-check failure makes the gate red; it cannot pass
as a skipped build. The required Release Risk check name and acceptance of the
Apple job's successful result remain unchanged.

Old evidence without these snapshots and execution sidecar is intentionally
ineligible, including PR14 run `39073373973` attempt `2`. It is not backfilled.
The first PR introducing this code must run a full native build; offline
fixtures do not replace that integration check. Reuse proves that historical
execution evidence applies to the same source and workflow contract, not that
moving `stable` tools were freshly resolved at merge time. A changed workflow,
source tree, contract or internal/external hash requires a new build.

Scope stays unsigned device-arm64 App plus PacketTunnel compile/link. The shared
component still has two arm64 slices, which does not imply a simulator carrier
was run. `devicePassed`, `signatureTrustValidated` and `publicationEligible`
remain false. Signing, device execution and publication requirements are
unchanged. `test_apple_merge_reuse.py` exercises complete fixture packages and
mutates identities, scopes, contracts, actual byte pins, archives, API routes,
redirect credentials and missing evidence; `apple-ci-impact.test.mjs` exercises
the full/reuse/fallback workflow result combinations. The existing strict
same-run consumer/import and aggregate controls remain in the test gate.


Execution-input collection and the complete host test gate require Python 3.11+
with standard-library `tomllib`; both the Release Risk classifier and Apple
producer explicitly select Python 3.12 and check/report the actual interpreter
and parser before use. The Apple setup stays after Xcode/SDK host admission.
The Linux resolver/sealer validates recorded configuration pins and does not
parse ambient Cargo configuration. A standalone producer without `tomllib`
remains explicitly ineligible and completes its ordinary full build; it never
creates a reusable execution sidecar. Unsupported-input controls exercise that
path separately; the supported snapshot/profile controls require real parsed
inputs and cannot pass merely because both snapshots became unavailable.
See https://docs.python.org/3.12/library/tomllib.html . Historical and ordinary
build-entry requirements are unchanged.

Revision closure and Cargo admission:

- The verifier joins each native receipt pin to both the transported payload and
  its retained original. Bind logs, every compiler object and external archive,
  generated-source and framework inventories, local replacement source snapshots,
  and overlapping retained files must agree in path, bytes, mode and type.
  Static member/origin tables are recomputed from bounded archive bytes using the
  existing parser. Original absolute paths are never opened on the verifier host.
- Execution snapshot v2 binds the Cargo environment before/after each producer
  phase. The actual Cargo hook also records its effective environment after the
  source-owned compiler/output/native-flag assignments. Values are length/hash
  pairs, never raw environment text. Registry tokens/credential providers and
  HTTP transport settings are omitted, including their hashes; no credential
  file is read. Wrapper/compiler/linker/runner selectors, alternate lockfiles,
  ambient compiler flags and unsupported external configuration cannot qualify.
- Cargo's working-directory/ancestor and Cargo-home configuration locations are
  checked. Only regular, tracked repository/root-or-src-tauri Cargo configs are
  admitted and byte-pinned. Includes, external sources, environment injection and
  unretained tool/flag inputs are refused. The initial flag policy supports the
  existing tracked static-CRT and debug-strip flags; changing this admission
  policy needs a reviewed source change. Legacy `config` and `config.toml` are
  both checked conservatively, including when Cargo would shadow one.
- `CARGO_BUILD_JOBS` and jobserver/Make FD handles only schedule existing build
  work and are not semantic pins; the existing native budget still governs them.
  Temporary paths are not added to the Cargo namespace. No environment variable
  is cleared or changed for qualification. Main verifies the original Mac
  execution contract; it does not compare it with the resolver's Linux home,
  temporary directory, run ID or jobserver descriptors.
- Unsupported Cargo inputs produce an explicit non-eligibility observation and
  the ordinary full build continues unchanged. The successful aggregate/chain
  remains mandatory. Sealing exports `eligible=false` and creates no execution
  sidecar, so a later main merge rebuilds. A supported contract must upload
  successfully. Missing/corrupt evidence, unexpected statuses or altered
  receipt/hash bindings are not silently converted to producer success.

Cargo input/configuration rules are audited against the primary references:
https://doc.rust-lang.org/cargo/reference/environment-variables.html and
https://doc.rust-lang.org/cargo/reference/config.html . The bounded, deliberately
conservative admission policy above is this repository's policy, not a claim
that every possible Cargo configuration is reusable.


Qualification never short-circuits integrity checks. The sealer collects valid
unavailable observations, validates every remaining snapshot and all comparable
phase/tool/SDK/environment bindings, then verifies all native/Rust/IPA/link
originals. It next compares the current chain and all five local summary files
with the uploaded bytes and checks the provider workflow binding. Only after
those checks may it return `eligible=false` without a sidecar. Malformed later
snapshots, missing/changed originals or changed current aggregate inputs remain
hard failures even when some or all execution observations are unavailable.


Carrier execution snapshots enter the existing producer `private_dir` boundary
before writing either supported or unavailable observations. A fresh carrier slot
is created with mode 0700 and the current owner; a preexisting wrong-mode/owner
or redirected slot is rejected, never chmod-repaired. The same slot is admitted
again by the actual carrier entry. Generic resolver/sealer output parents retain
their existing semantics; no global directory-permission relaxation is applied.
