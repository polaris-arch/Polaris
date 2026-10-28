# Native node DNS: final desktop selection and validation boundaries

The native node DNS generator requires the corresponding kernel implementation.
Source-only success, an absent kernel or a previous kernel cannot establish the
new policy's runtime compatibility.

## Published alpha.12.6 selection

The final bfcdf96 source is selected using actual public immutable Release 425664135,
published at 2026-10-27T15:58:25Z, and the four independently matched public
asset identities below. The original All producer index, exact automatic source
archive, annotated tag and producer identity are bound in one exact contract.
The prior alpha.12.5 / 332935 desktop contract/index and source evidence remain
unchanged at App main 311ce7a407c9478b09b43ddff7e7cd81f4737928 and in nested history.
Its delivered DMG retains that exact App/core identity, not alpha.12.6.
The prior alpha.12.4 / 0cfa desktop contract/index and source evidence remain unchanged
at App main f236f4001da3a688e22aa09fdfd1b5df2511b179 and in nested historical source evidence.
The delivered ec422d DMG remains its original App/core identity, not alpha.12.6.
The prior c6 desktop contract/index and source evidence retain their original identities
in Git history and the nested historical source evidence; they are not relabeled as bfcdf96.
The prior 814 desktop contract/index and observations remain unchanged in Git history at App commit `bce3056c600f1738468acb7810cf2c086279aa5a`; they are not relabeled as c6.
The a4d085 desktop contract and observations remain available unchanged at App
commit 42706a1fc096b91f9a4186c0a4c68b0cdc88591b; the older c002/fe182 material and
mobile a4d snapshot retain their own identities. No prior package, runtime or
mobile acceptance observation is relabeled as this delivery.

Source, producer and public metadata establish input provenance, not App runtime
acceptance. Do not overwrite this immutable Release, its tag or its assets to
repair an App consumer failure.

## Selected release and independently preserved history

`src-tauri/core-manifest.json` now selects `desktopRelease` through the exact
canonical `scripts/desktop-core/release-contract.json` bytes:

- Repository: `polaris-arch/sing-box-polaris`.
- Source: `bfcdf96b244f63e0fe84736ee4a4edd58f6b209e`.
- Tree: `4c0fdb31d59967f41c808ea6b63f87ca9661c2cf`.
- Version/tag: `1.15.0-polaris-alpha.12.6` / `v1.15.0-polaris-alpha.12.6`.
- Annotated tag object: `8a230c1d34cb4855b261944e2935ce36f9cd14de`.
- Public asset names use `sing-box-polaris-1.15.0-alpha.12.6-*`; the tag,
  original archive filename/root and BuildInfo retain `1.15.0-polaris-alpha.12.6`.
  The original producer index and provenance retain their historical name
  annotations; public renaming changes neither their bytes nor payload pins.
- Public prerelease: `425664135`; producer run `40478887755`, attempt 1.
- Exact four archive IDs: Linux `682088894`, Windows `682077119`, Intel macOS
  `682082597`, Apple Silicon macOS `682089262`.

This is a real upstream-layout formal release contract, not a relabeled staging
index. The exact historical 577d contract is retained as
`historical-release-contract-577d4b9.json`. The unchanged fe182 temporary contract,
source binding and historical manifest selection retain release `416260275`,
producer `39075665574`, original null staging-index tag object and their original
proofs. Neither historical selection can replace the current compiled identity.

## Four public packages, committed index, fixed source archive

The consumer requires the four original desktop package assets. Linux/Windows
Cronet companions are already inside their packages; macOS uses static linkage.
No separate public Cronet, provenance, NOTICE, native index or custom source
asset is needed by this selection.

The entire unchanged original index is committed at
`scripts/desktop-core/release-asset-index.json`, with SHA-256
`5ddcb1b623492bc555d7fcc0bc17798ea16200df21d2207cfd9a228ffc340caa`.
The consumer checks its byte count, hash and all selected original rows against
the canonical contract. It does not obtain the index from cross-repository
Actions credentials or accept a replacement environment path.

Source transport is the fixed commit URL
`https://github.com/polaris-arch/sing-box-polaris/archive/bfcdf96b244f63e0fe84736ee4a4edd58f6b209e.tar.gz`.
The actual 19,747,580 bytes have SHA-256
`70d0bed894f62b557a37c5f3a9211ced21807e135f144138189663ff2fc73566`.
Only its exact same-commit codeload redirect is admitted. The existing archive
verifier checks the complete expanded Git tree, separately observed gitlinks,
module hashes, the v3 source policy with all ten reviewed local replacement trees
(2,876 tracked files), and every
original legal-file byte/hash. The original producer custom-source hash remains
in `producerSourceArchive`; it does not attest these different transport bytes.

Public repository/release/tag/source/run/native-job metadata remains verified
before and after acquisition. Existing CI API credentials are confined to the
canonical API origin; public source/package redirects receive no Authorization.
No new credential, cross-repository permission or public sidecar is introduced.

## Shared App and packaging route

Ordinary main/PR/build/test/package selection uses the committed formal contract
without requiring a manual branch or environment selector. `fetch-core.mjs`
continues through `consumeRelease`, the original strict archive validator and
normal `resources/<platform>` layout. Original archive members and generated
legal/provenance/source metadata remain distinct verified domains. Partial,
extra, mixed-source or historical carriers fail closed.

CI/package stages its actual helper and Windows cleaner, with the existing
signing-before-freeze order, before `prepare-core-identity.mjs`. Preparation
freezes those actual bytes; later Cargo rebuild output cannot redefine them.
The installation provider fingerprint is SHA-256 of
`polaris-upstream-native-installation-v1\0` plus the exact validated contract
bytes. `build.rs`, source-v2, selected manifest, normal resolver, Windows
expected_source, helper CoreID, cleaner/install and updater all use that same
selected identity. Arbitrary environment values cannot repin it.

Analysis-only builds without a prepared identity use a distinct unavailable
sentinel. It cannot resolve a core or authorize a service. A present, explicit
or incomplete installation must fail verification rather than select a sentinel
or old formal fallback. Missing package resources are not runtime acceptance.

Ordinary CI reuses its existing full Rust/UI commands and the same-commit
four-host workflow. Required config, schema, dependency, wire, TLS, Cronet and
API gates consume exact selected resources and fail on missing binaries.
Existing lifecycle/stats fixtures use the compiled default resolver without a
kernel-path override, then the existing per-instance test spawn opt-in. The
normal full test matrix is Linux amd64, Windows amd64 and macOS arm64; Intel
macOS retains its native/package routes. See `native-dns-runtime-gaps.md` for
platform-specific lifecycle coverage, including Windows boundaries.

Windows directory tests use the selected clean source's `release/LDFLAGS`,
selected production tags/version and actual SourceRevision. This is explicitly
a directory-test build recipe, not an invented package-producer `buildCommand`.
The test receipt retains protocol-only scope. Existing SYSTEM permission and
event conditions are unchanged; an unexecuted SYSTEM gate is not a pass.

## Check-only identity and evidence still to collect

Linux protected checks share the same PlainTcpCheckV1 execution semantics,
config whitelist, budgets, seals, source replacement protections, standard-FD
controls and cleanup. Historical B609 and fe182 rows retain their identities and
evidence. A third private c002 row binds the actual Linux ELF SHA-256
`b754ac8223b27f4c20b40ced82d9ad5d58fee8fb4c5bfa48c38566e805c570ae`
to the exact compiled c002 source and canonical contract digest. No environment
or caller-provided hash can register a row.

A fourth private a4d085 row records the independently verified producer ELF
`2a7495faeb18d2666dfcc77f678057c35cc7a7bb36b03a8a16ff9ac6249b4db8`.
Its exact compiled source and canonical contract digest select the same
PlainTcpCheckV1 semantics. No compiled source/hash pair can borrow c002 or fe182
admission. Sealed-runtime evidence remains pending.

The fifth private 814da row remains historical and cannot match the new compiled contract.
The sixth private c6c0d8 row retains its historical source, contract and
Linux ELF `80e21e78875f2f1cbfb1356dd977d6043a7776179df61f57897a4d811eaa1eaf`.
The seventh private E0CFA24 row retains the historical 0cfa source, contract
`96e8cc61d7abf345145b2af230724708f9a1baea77715467e38e96277ae0ffdf`
and Linux ELF `7fde19e331e654280e134a89c26af5be706e4364e3003a74b7619c220d5a03aa`.
The eighth private E332935 row retains its historical alpha.12.5 source/contract
and Linux ELF `d34fd051f8e089610bf32e58a5cf95ac36411827e46845157777baba078893ad`.
A ninth private BFCDF96 row binds the current bfcdf96 source and canonical contract
`66894ed01e8fb8f40b90e11427c0041fec3dc1e69be4439cd9ac0dbc8c849e5f`
to Linux ELF `9089e0a78616d714eeb63d052dff8505a8b21ed9ba24285ed7444067cde69988`.
It uses the same PlainTcpCheckV1 semantics, rejects cross-paired historical
sources/contracts and explicitly records runtime evidence pending. It permits the existing
Linux Test gate to collect its own sealed check evidence; no prior probe is
claimed or required as a circular prerequisite to that gate. Check success never
grants managed launch. The closed managed opt-in proposal remains Unsupported
and is not a dependency of ordinary native birth.

The four public archive metadata sizes/IDs/API digests match the reviewed final
producer inventory and immutable Release. Public archive body downloads and their
static payload verification are pending the next formal four-host App consumers;
API equality and producer upload checks do not claim those downloads occurred. Their payload hashes, raw/parsed Go BuildInfo,
module graph, source, tags and producer observations remain the original index
facts. This source change does not claim a fresh App download, ELF execution,
Mac signature observation, Rust pass, sealed execution or final-package runtime
acceptance. Those must be collected by the existing actual native consumers.
Earlier four-host success on fe182, a4d085 or another App commit does not transfer
to bfcdf96. Schema fixtures cannot be regenerated merely to obtain a pass.

## Mobile and publication boundaries

Mobile source-contract independently binds the same actual bfcdf96 source/tree/tag,
Go 1.27.0 directive, Go 1.27.2 toolchain, exact module hashes, the v3 source policy's
ten reviewed vendor trees and libbox source/API declarations. Its raw SHA-256 is
`8acececd46ddbe2f12056a15b6009400044508f2c162781f86ef2b63fee01104`.
Unknown output receipts remain null and acceptance flags remain false; neither
a public desktop ELF nor a successful kernel SDK producer can fill them.

The prior All producer's genuine 814 internal SDK artifacts remain historical
814 evidence, not observations of the new bfcdf96 SDKs. They are not App formal
`polaris-mobile-component-v1` inputs: Android artifacts are separate single-ABI
AARs, Apple lacks the required iOS-arm64 simulator slice, and their empty BuildID
and producer receipts do not satisfy the current App input-fingerprint binding.
They are not relabeled old diagnostics, but must not be combined or re-signed
as an App receipt. Reuse source/tool evidence and normally admitted tool/module
caches, then run the existing App formal producer once for its exact profiles.

The Android producer retains API 24, all four ABIs and the existing 16 KiB contract
for every native ELF, plus generated Java/JNI HOT symbols and actual Kotlin
compilation/fixtures. The Apple producer retains exactly iOS-arm64 and
iOS-simulator-arm64, actual ObjC/Swift importer/host compilation, and the existing
PacketTunnel lifecycle checks. RuntimeCapabilities, all six runtime RPCs,
Retryable start and RuntimePersistenceHandler remain strict consumer gates.
A toolchain build, compile-only host or JVM fixture does not prove callback,
link/load, device execution or signed carrier acceptance.

`pending-release-selection.json` deliberately retains `AppAccepted: false`.
The existing publication guard admits this exact reviewed canonical formal
contract and its strictly validated selection/index as source eligibility only.
A different contract requires renewed review; temporary or malformed selections
cannot inherit this admission. The historical source-first branch remains
separate. Existing signed Release DAG CI, producer/consumer, tag, signature,
asset-digest and promotion checks still execute. Native/device acceptance remains
NotObserved, Exact/NoOwner Unknown and managed NotGranted. No new acceptance
receipt or launch permission is introduced. Neither source eligibility, producer
native smoke nor static verification invents final App/device acceptance.

## Alpha.12.6 cleanup regression boundary

Review the six existing controlled cases against the exact selected source/core hash.
For full lifecycle final stop, the explicit-restart successor final stop in the
hot-switch-failure case, and aggregate relay stop, retain raw stdout/stderr with
owned PID/generation and the final-stop interval. Reject unexpected core cleanup
errors, including fake-IP database-already-closed failures, in that interval; do not
misattribute the earlier intentional SIGKILL or other injected-failure output.
Process reap and App shutdown-fence evidence remain a separate layer. The current
CLI prints resource snapshots only on Close error, so silence and exit0 are not a
positive zero-failure core resource receipt. The kernel's actual CloseWithResult,
resource-snapshot and database-reopen regression tests retain their own source/test
identity; they are not substituted for a new App flow or device acceptance.

The alpha.12.6 CLI explicitly binds the existing Tailscale child registrar before
ordinary `run`/`check` construction. The reviewed constructor regression and
source-bound tests do not establish real Mac startup recovery. The final new
App package must contain this exact source/core, and real startup/stop remains
a separate observation; the previous failing 311ce7 / 12.5 package is not rebuilt
or relabeled as fixed.
