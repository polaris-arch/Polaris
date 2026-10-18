# Native DNS candidate validation and remaining gaps

The existing manual `ci.yml` entry calls the local reusable `Native DNS Candidate`
workflow from the same commit to validate temporary desktop inputs on
`collab/native-node-dns-20261018`. This works before the new workflow is present
on the default branch. Its required candidate input must equal the caller's
manual workflow SHA and checked-out HEAD. It runs on native Linux amd64,
Windows amd64, macOS Intel and macOS arm64 hosts, with read-only repository
permissions and public pinned downloads. It does not install a system service,
use administrator elevation, select a formal release or produce installers.

The kernel source is `fe182431495711c09bdb3b074ed6aa6309c00cf8`, tree
`9b2fe41af73c5768dccff079ded1e4a489cfa9e1`. The temporary publication does not
rebind the existing alpha.12 version tag. Acquisition receipts attest input
bytes and context only; they do not attest runtime success.

## Runner prerequisites and final rebase

The setup follows the existing desktop workflow: Node 26, Python 3.12,
preinstalled rustup installing Rust stable, pinned protoc 35.1 through
`scripts/fetch-protoc.mjs`, and Go from the verified candidate source policy
(1.27.2 for this source). Go is required to inspect actual BuildInfo and build
IDs; no replacement kernel is compiled. The resolved Go version and native
Rust host triple are checked. macOS checks its existing SDK and clang paths
using the same `xcrun` steps as `desktop-core.yml`; no SDK or security setting
is changed.

The selected Cargo targets belong only to config-engine, mesh, net-stack and
singbox-grpc. Their locked transitive dependency closure was inspected: it
contains no Tauri, GTK/WebKit, OpenSSL, btls-sys/BoringSSL, CMake or NASM build
dependency. The gRPC transport is h2c without TLS features. Consequently this
workflow needs neither UI/pnpm installation nor the privileged Linux system
package setup or Windows NASM setup used by whole-workspace/App builds. It
retains the normal native linker supplied by the hosted runner. Widening the
Cargo target set requires a fresh prerequisite audit; this is not an App-build
toolchain claim.

The integrated base is main `f076d3da055581cdf004d0d5147e712b44065a16`,
which already contains PR #15's reviewed toolchain and symlink-context fixes.
Final execution must bind the final reviewed App commit, never this historical
base or an earlier source-review tree.

## What the runner establishes

All four payloads are verified before and after the gate series. The uploaded
input evidence records their member hashes, BuildIDs, Go versions, candidate
context and kernel source/release identity; the final summary hashes that file.
Each host runs
the existing dependency, build-matrix, schema, generated-outbound, full-config,
network-profile, subscription-guard, userspace mesh and managed CONNECT gates.
The generated-outbound target retains its TLS checks. The existing Tailscale
login-config and three import-protocol checks also run. The all-platform gRPC
wire gate and the explicitly selected ignored native Clash API test are
mandatory. Linux and Windows additionally run the existing native Cronet C API
and actual loaded-library mapping proof against their canonical companions.

Each command has its own log and result. A failure does not prevent independent
later gates. A missing executable, missing test, zero-test success, ignored test,
timeout or skipped required gate cannot produce an overall pass. Unsafe child
cleanup stops further execution and marks remaining gates blocked. The series
has a 150-minute budget, each command at most 20 minutes, and bounded logs.
Setup failures and interrupted runs retain an incomplete summary. Workflow
artifacts include logs and the summary even when validation fails.

The network-profile harness deliberately excludes its DHCP packet-safety case
outside Linux. That exact exclusion is recorded, never represented as DHCP
coverage on Windows or macOS. Linux retains the harness's existing unprivileged
safety precondition and required-gate failure behavior.

Schema or behavior drift remains a failed gate until independently reviewed.
The runner does not regenerate schema snapshots, suppress diagnostics, reduce
full configurations, weaken fixture ratchets or reinterpret prior failures.
No workflow or runtime success is claimed merely because these files exist.

Even if every ordinary gate passes, the runtime summary keeps `appAccepted`,
`packagedAcceptance` and `publicationEligible` false. These scoped tests do not
establish packaged application loading, final-carrier identity or device
acceptance. Fresh App-generated policy fixtures must also be consumed by the
kernel's `TestAppGeneratedNodePolicyDialAndListen` and
`TestNaiveCallbackUsesActualNativePolicyForHTTPS` tests and broader native policy
regressions. Their controlled transports are semantic evidence, not device
acceptance. Mobile AAR/XCFramework production and consumption remain separate.

## Ordinary full-App execution and platform coverage

The existing `ci.yml` has an optional, default-empty `native_dns_candidate`
input. See `native-dns-consumption-plan.md` for exact admission and compiled
carrier construction. It retains Node 26, Rust stable, pinned protoc/NASM,
Linux GTK/WebKit dependencies and the original workspace commands. Candidate
jobs additionally use Python 3.12 and verified Go 1.27.2. The existing UI gate
remains separate and unchanged. No duplicate full-App workflow is introduced.

All ordinary test jobs verify the exact candidate inputs and compiled carrier
before and after the workspace tests. Logs and the real installation manifest
are retained with the App SHA/run/attempt and acquisition receipt. Ordinary
nonignored tests are not evidence that ignored tests ran. Candidate lifecycle
and stats tests reject any `POLARIS_SINGBOX_PATH`, call the normal default
resolver/content verifier, assert the exact compiled carrier, and print its
path, CoreID and source. The returned carrier is admitted by the existing
per-instance real-core test gate; there is no global test spawn fallback.
The selected command
is `cargo test --locked -p polaris --lib <exact name> -- --exact --ignored
--nocapture --test-threads=1`, with exactly one passed test required per log:

- Linux amd64, macOS arm64 and Windows amd64: `runtime::proxy::tests::lifecycle::real_core_full_lifecycle`,
  `runtime::proxy::tests::lifecycle::real_core_lifecycle_race_start_then_immediate_stop`,
  `runtime::proxy::tests::hot_switch::real_core_hot_switch_keeps_pid`,
  `runtime::proxy::tests::hot_switch::real_core_auto_failover_attests_without_applying_saved_debt`,
  `runtime::proxy::tests::hot_switch::real_core_hot_switch_failure_falls_back_to_restart`.
- Linux amd64, macOS arm64 and Windows amd64:
  `runtime::stats::tests::real_core_tests::real_core_aggregate_relay_emits_real_frames`.
- Intel macOS: the separate four-host config/wire/TLS/Cronet/API plan applies;
  the unchanged ordinary matrix does not run a full-App Intel job.
- Windows uses native process handles for liveness and an exact case-insensitive
  `sing-box.exe` ToolHelp process census. OS query failures fail the tests rather
  than reporting absence or zero orphans. Crash injection retains the existing
  held-process `NativeStaleCoreIo` path. The same five exact proxy tests now run
  on the ordinary Windows leg without the former POSIX fixture exclusion.
  The summary marks their requirement satisfied only after all five logs show
  exactly one passed test and matching passed status records. Missing, skipped,
  interrupted or failing execution remains open; wiring alone is not acceptance.
  The separate user-token directory and SYSTEM routes retain their existing
  scope and guards. No SYSTEM acceptance is inferred from these tests.

The existing ignored recovery tests (`real_core_crash_triggers_auto_restart`,
`real_core_crash_feeds_diagnostic_restart_axis`,
`real_core_intentional_stop_does_not_restart`,
`real_core_crash_loop_gives_up_without_infinite_restart`) and
`process_supervision::real_core_stale_cleanup_kills_own_orphan_spares_foreign`
are not newly selected by this bounded route. They remain separate recovery/
orphan requirements, not implied by ordinary cargo success. The six selected
tests cover direct unprivileged App lifecycle/hot-switch/stats only; they do
not authorize service installation, TUN/system-proxy changes or managed launch.

## Separate sealed check identity and its evidence boundary

The old B609 digest/profile remains intact. A separately named
`Fe182PlainTcpCheckV1` now pins Linux x86-64 digest
`7420d9777154754b9182a4269d12348544d925269b700a4f34a29647a4155399`.
It is internal check/preparation metadata, not a helper wire execution profile.
The original positive tests explicitly select this profile only in admitted
candidate test mode; malformed selectors and missing required input fail.
Profile substitution and wrong binaries reject before execution.

One prior exact native observation ran only the direct-outbound JSON through
sealed binary/config memfds, child config fd 0, empty environment, root cwd
and `--disable-color check -c /proc/self/fd/0`. It exited 0 with no output and
seals 15 retained. That establishes the narrow sealed check mechanism only.
The full-builder VLESS/plain-TCP whitelist is unchanged. Its full positive/
negative Rust matrix still needs actual CI results: full bytes, cache/listener
states, binary/profile identity, snapshots, cancellation, source replacement,
closed descriptors, timeouts and reaping. No VLESS/TLS/Naive acceptance is
inferred from the direct-only observation. The 96 MiB budget and loader trust
boundary are unchanged.

Helper `exact_start_capability` remains `Unsupported`; checked evidence cannot
grant launch authority. Mutable cache-writer handoff, selector readback, frozen
runtime facts and exact managed birth remain separate obligations. Existing
strict-check/preparation APIs have unit-test callers, so this limitation must
not be misreported as a missing kernel patch or silently converted to launch
permission. Mobile AAR/XCFramework and device acceptance remain unverified.
