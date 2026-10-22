# Native node DNS: final desktop selection and validation boundaries

The native node DNS generator requires the corresponding kernel implementation.
Source-only success, an absent kernel or a previous kernel cannot establish the
new policy's runtime compatibility.

## Published alpha.12.1 selection

The final 814 source is selected using actual public immutable Release 421066315,
published on 2026-10-22 at 18:47:54Z, and the four independently matched public
asset identities below. The original All producer index, exact automatic source
archive, annotated tag and producer identity are bound in one exact contract.
The a4d085 desktop contract and observations remain available unchanged at App
commit 42706a1fc096b91f9a4186c0a4c68b0cdc88591b; the older c002/fe182 material and
mobile a4d snapshot retain their own identities. No prior package, runtime or
mobile acceptance observation is relabeled as this delivery.

Source, producer and public bytes establish input provenance, not App runtime
acceptance. Do not overwrite this immutable Release, its tag or its assets to
repair an App consumer failure.

## Selected release and independently preserved history

`src-tauri/core-manifest.json` now selects `desktopRelease` through the exact
canonical `scripts/desktop-core/release-contract.json` bytes:

- Repository: `polaris-arch/sing-box-polaris`.
- Source: `814da362838e96e54f904836a92c2b0d8c3ae29b`.
- Tree: `ad574f93026557b49edecb492bd7ecdb851057c1`.
- Version/tag: `1.15.0-polaris-alpha.12.1` / `v1.15.0-polaris-alpha.12.1`.
- Annotated tag object: `8ecda357ea755a1295336a35ec7eaf1fe2daebc2`.
- Public asset names use `sing-box-polaris-1.15.0-alpha.12.1-*`; the tag,
  original archive filename/root and BuildInfo retain `1.15.0-polaris-alpha.12.1`.
  The original producer index and provenance retain their historical name
  annotations; public renaming changes neither their bytes nor payload pins.
- Public prerelease: `421066315`; producer run `39787021317`, attempt 1.
- Exact four archive IDs: Linux `666747193`, Windows `666748404`, Intel macOS
  `666739498`, Apple Silicon macOS `666754500`.

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
`1b5c1ae5a83a452cd14cda6ea608f3964f4e1ee1ee1face98b40ee400fe47cb5215`.
The consumer checks its byte count, hash and all selected original rows against
the canonical contract. It does not obtain the index from cross-repository
Actions credentials or accept a replacement environment path.

Source transport is the fixed commit URL
`https://github.com/polaris-arch/sing-box-polaris/archive/814da362838e96e54f904836a92c2b0d8c3ae29b.tar.gz`.
The actual 12,673,355 bytes have SHA-256
`f66a790d43b7a229299928d521141a99588b80535fe2dea3445f3bc436b96fe9`.
Only its exact same-commit codeload redirect is admitted. The existing archive
verifier checks the complete expanded Git tree, separately observed gitlinks,
module hashes, all nine reviewed local replacement trees (2,838 tracked files) and every
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

A fifth private 814da row binds the current source and canonical contract to
Linux ELF `363261029cea32333adcdd67288b23e4ae4084efa96b7f2b157af6e9d39946ee`.
It rejects cross-paired historical sources/contracts and explicitly records runtime evidence pending. It permits the existing
Linux Test gate to collect its own sealed check evidence; no prior probe is
claimed or required as a circular prerequisite to that gate. Check success never
grants managed launch. The closed managed opt-in proposal remains Unsupported
and is not a dependency of ordinary native birth.

The four public archive bytes/IDs/API digests match the reviewed final producer
inventory and immutable Release. Their payload hashes, raw/parsed Go BuildInfo,
module graph, source, tags and producer observations remain the original index
facts. This source change does not claim a fresh App download, ELF execution,
Mac signature observation, Rust pass, sealed execution or final-package runtime
acceptance. Those must be collected by the existing actual native consumers.
Earlier four-host success on fe182, a4d085 or another App commit does not transfer
to 814. Schema fixtures cannot be regenerated merely to obtain a pass.

## Mobile and publication boundaries

Mobile source-contract independently binds the same actual 814 source/tree/tag,
Go 1.27.0 directive, Go 1.27.2 toolchain, exact module hashes, nine reviewed vendor
trees and libbox source/API declarations. Its raw SHA-256 is
`cf3bdf9999f99ae180d0b1e3ce6499e1ae6251f682216ae5f0cb1570226869e1`.
Unknown output receipts remain null and acceptance flags remain false; neither
a public desktop ELF nor a successful kernel SDK producer can fill them.

The All producer's genuine 814 internal SDK artifacts are not App formal
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
