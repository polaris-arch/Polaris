---
status: active
updated: 2026-09-30
area: polaris
---

# Android native admission coverage (protocol 1)

`AndroidNativeAdmission` is a process-local safety ledger. It is not a migration
receipt by itself. The Kotlin `wiredProducers` set derives from installed adapter
capabilities; each identifier enters it only after the corresponding production
path and its late/timeout tests are present. Rust rejects every incomplete manifest and never converts
the shape verifier's `Ok(())` into `NoOldCore` or custody release.

| Producer | Entry and captured work | Terminal proof required | Late, timeout, or cancellation path |
| --- | --- | --- | --- |
| `main.bridge` | Plugin Start before foreground-service dispatch; one ticket continues from `VpnBridge` pending into `MainKernelAttempt` | `CancelledBeforeBirth` or exact close | Rust bridge timeout detaches but Kotlin ticket remains; late Service attempt must meet sealed admission |
| `main.system` | `BoxService.onStartCommand` for Boot, always-on, system relaunch, and duplicate foreground intents | same main owner proof | fence, request replacement, or occupied registry rejects an ownerless Service; `stopSelfResult(startId)` tears down only that Android request |
| `main.close` | exact attempt close after Stop/onDestroy/onRevoke | construction completed normally, native `CloseService` and `Close` succeeded, then registry released the same attempt, then `ClosedExact` | native construction failure/revoked completion is sticky `Unknown` under `operationLock`; timeout, failed close, or exceptional future cannot become exact |
| `main.reload` | Service reload callback, including targetless callback | control operation leaves JNI; same main ticket stays owned | retired endpoint declines before native reload; late callback cannot start after seal |
| `login.start` | owner reserve at invocation, independent validation reserve before worker queue, owner birth before setup/JNI | capability absent; born owners remain `Unknown` after ordinary cleanup | consumed ID cannot replay; cancelled/sealed worker cannot enter native; construction failure/revoked return is sticky `Unknown` under ownership lock |
| `login.close` | explicit close and Rust Drop use original ID; serviceStop, 300 s expiry, retry and main preemption retain exact Entry | worker exit, native close, network/DNS and original cache/TS ownership proof remain pending | close-before-start tombstones ID; successful map removal is operational cleanup only and cannot prove TS release |
| `speedtest.start` | ticket installed before pure config-validation thread and session queue; `enterBirth` before prepare's first resource | exact speedtest owner close; capability still absent pending resolver drain | its epoch:sequence ID cannot be replayed; cancelled/sealed queue cannot enter native; native construction failure or revoked completion stays ledger `Unknown` |
| `speedtest.close` | same ticket for explicit close, serviceStop, expiry, main-start preemption | native close, original prepare/Start worker exit, network notifications/thread cleanup, and unused resolver shutdown | 8 s timeout leaves owner observable; used DNS resolver stays ledger `Unknown`; operational Close success preserves ordinary retry/main admission |
| `validation.checkConfig` | common enqueue adapter reserves before plugin thread, login worker queue, and speedtest validation thread/session queue | `CancelledBeforeBirth`; otherwise `ValidationCleanupUnknown` until Go returns an observable construction/disposal proof | failed queue dispatch cancels before birth; Rust timeout/detached callback does not end the Kotlin JNI operation |
| `control.targetlessStop` | old targetless Stop callback before dispatch | control callback leaves its work; main owner remains until exact close | queued/late callback cannot disappear from a sealed receipt |
| `control.targetlessReload` | old targetless reload callback before dispatch | callback returns after any JNI work; main owner remains | reload rejected by seal cannot enter new native work |

Cold-process bootstrap is shared by all rows: only `ENOENT` for
`mesh-route-state.required` opens admission. Marker presence or any other stat
failure keeps the process closed. Seal is permanent for this process; a matching
fence may reread the frozen captured set, while another fence is rejected.

The integrated conservative `construction-validation.patch` adds the typed Go
`CheckConfigWithResult` API. Legacy `CheckConfig` still returns only the validation
error, independently of cleanup uncertainty. The typed API binds request ID,
config digest and contract version; parse rejection may report `NoConstruction`,
but actual construction, partial failure and timeout remain `CleanupUnknown`.
`DisposedExact` is reserved and is not emitted for constructed boxes. Timeout
does not prove that its rollback worker has ended. Android still calls the
legacy API and does not consume a typed disposal proof, so entered JNI validation
remains `ValidationCleanupUnknown`, even after Java returns. Neither this typed
shape nor an ordinary `Close() == nil` establishes exact native release.
Main Start/Reload has the same partial-construction risk. A construction failure
or cancelled completion records `Unknown` while holding the same attempt's
`operationLock`, before Stop's close worker can settle it. A successful ordinary
CommandServer close cannot clear that fact. Exceptional/cancelled preparation,
close, or registry-release futures also remain `Unknown` without throwing from
the settlement callback.

Current wiring remains **4/11**: `main.bridge`, `main.system`, `main.close`, and
`validation.checkConfig`. Speedtest and Login owner tickets are installed, but
their capabilities remain empty until their remaining cleanup gates are proved. Other
rows describe required contracts. Deterministic JVM seam tests cover both Stop/construction
failure orders, revoked JNI completion, old-A/new-B settlement, prequeue seal
and queue failure at all three validation ingress paths, request replacement
before/after admission, and exact-startId teardown versus a successor request.
The Android startId comparison is documented by [Service.stopSelfResult](https://developer.android.com/reference/android/app/Service#stopSelfResult(int));
this change has no APK or device validation.
No partial implementation may clear `historicUnknown`,
Tailscale custody, or enable managed routing.

Validation of the preceding main/Service follow-up (2026-09-30): full
`:app:testUniversalDebugUnitTest` passed 83 tests in 11 suites, with no failures,
errors, or skipped tests. `node scripts/check-android-bridge.mjs` passed all
31 bridge commands; `git diff --check` passed. The JVM Service seam models
Android's documented startId comparison; it does not substitute for device FGS
lifecycle testing.

Speedtest owner follow-up: a single ticket follows the original epoch:sequence
from the first queue through explicit close, serviceStop, expiry, and main-start
preemption. Close may cancel an in-flight Start, but terminal settlement waits
for that original worker to pass its last native resource acquisition. Native
factory/box.New failure or revoked completion is sticky ledger `Unknown`, even
when operational Close succeeds. Real native Close or network-unregistration
failure keeps the existing operational `cleanupUnknown` owner and blocks retry.

Network notification callbacks are fenced and joined, and the instance's
HandlerThread must exit before an unused resolver may contribute exact cleanup
proof. A queried resolver is sealed against new queries and drops later writes
through its original ExchangeContext. A JNI write that passed the close flag
before sealing can still finish on that old context, so queried cleanup remains
`Unknown`. This supplemental proof result does not turn an operationally
successful close into permanent active ownership: ordinary subsequent speedtest
and main Start retain their prior admission behavior.

**Pending P1 gate: queried resolver callback drain and disposal.** A future
independent slice must capture each SDK DNS query/cancellation, suppress late
context writes, join in-flight query/JNI work, and confirm that the callback
executor has ended and cannot accept late SDK deliveries. Callback return,
Go context cancellation, or an empty active-session map alone is insufficient.
This gate is mandatory before advertising `speedtest.start`/`speedtest.close`
coverage or using a queried resolver's disposal as an exact release fact. Native
Consuming a Go construction/disposal receipt remains a separate mandatory
Android proof gate; the current conservative contract cannot establish exact
release of a constructed box.

Validation of the Speedtest owner follow-up (2026-09-30): the targeted owner and
admission suites passed 37 tests; the full `:app:testUniversalDebugUnitTest`
passed 98 tests in 11 suites, with no failures, errors, or skipped tests.
`node scripts/check-android-bridge.mjs` passed 31 commands; `git diff --check`
passed. Latch tests exercise both Start-failure/Stop orders, queued seal and
close-before-start, late old-A/new-B completion, expiry/main preemption,
queried-DNS operational recovery versus ledger uncertainty, and actual Close
failure blocking retries. Foreign-family and foreign-ID ticket rejection
replies without retiring another owner. Android DNS/framework behavior has no
APK or device validation; queried resolver disposal remains the pending P1 gate
above, and coverage stays 4/11.

Login identity follow-up: `TransientLoginNativeOwner` holds one original ticket
from invocation through queue dispatch and Entry disposal. The independent
Validation ticket is reserved before the same queue. Config rejection, occupied
admission, rejected queue, close-before-start and seal may settle only a ticket
that never entered native birth. `construct` records failure or revoked return
before releasing the host ownership lock. ServiceStop, expiry, retry and main
preemption operate on the captured Entry; delayed callbacks never resolve a
successor from an ID lookup. Rust timeout, Drop and confirmed close preserve the
same config-file-stem instance ID; no identity was replaced with a new ticket.

This slice retains the previous operational cleanup and state-directory claim
behavior. Born tickets remain `Unknown` even when native/network/cache cleanup
succeeds and the operational map removes the Entry. Worker completion, strict
network cleanup, the queried resolver drain gate above, and ownership of cache
deletion versus Tailscale state-directory claims require subsequent proof.
Neither map removal nor main preemption is a Tailscale custody-release receipt.

ID tombstones and ticket metadata live for this Android process. Every observed
or explicitly closed ID remains consumed across later Rust attempt epochs, even
after operational disposal. A never-observed, never-tombstoned older epoch has
no cross-ID ordering proof here; its string alone cannot establish staleness.
A Rust-authorized generation protocol is needed for that boundary. No TTL or
recent-ID eviction window may silently reopen a consumed identity.

**P2 candidate for review: process-ledger metadata has no cardinality bound.**
The live operational map remains limited to eight Entries. Successful disposal
clears native/network/cache references; a captured expiry may still retain that
disposed Entry's ID and directory metadata until its scheduled callback/worker
drains. The ledger permanently retains ticket/state/ASCII-ID metadata, not Entry,
config, authorization URL, File or cache contents, and its count grows with
attempts. A conservative admission limit or epoch rotation needs an explicit
Rust-authorized protocol and should be assessed separately. This slice neither
evicts tombstones nor claims a bound or cleanup proof that does not exist.

Validation of the Login identity follow-up (2026-09-30): targeted owner,
validation and admission suites passed 27 tests; full
`:app:testUniversalDebugUnitTest` passed 110 tests in 12 suites with no failures,
errors or skipped tests. Latches cover close-before-start, queued validation
failure followed by old retry/expiry/Stop after successor birth, main preemption
before the old worker runs, both construction-failure/close orders and revoked
late success. A 2,050-attempt history does not reopen consumed epoch IDs. Kotlin
source checks cover exact Entry captures and original Rust ID propagation.
The unchanged `android_transient_login_wiring.rs` passed all six source-contract
tests using the real std-only `polaris-source-probe` library and a standalone
`rustc --test` invocation; this is not a full Cargo/runtime test. Bridge checking
passed 31 commands and `git diff --check` passed. No APK/device testing or Go
changes were performed, and coverage remains 4/11.

Login Host seam follow-up: the production `TransientLoginHost` delegates to
`TransientLoginHostState`, which owns the actual Entry map, state-directory
claims, predecessor disposal, worker dispatch, close callbacks and timers.
Fake-native tests run that same state machine with controlled queues and latches.
The production adapter retains native CloseService/Close, network close and
cache deletion in their prior order. Configuration is cleared from the adapter
after Start returns or disposal is attempted. Successful cleanup drops the
Entry's engine reference; failed cleanup retains it and blocks conflicting
Login/main work. Callback exceptions are isolated from dispatch and complete
each caller at most once; a failed retry timer cannot suppress the close result.

The tests pause a queued old worker while main preemption and a successor
overtake it, pause native Start while Stop waits for the ownership lock, and
deliver old expiry/retry/native Stop callbacks after a successor uses the same
state directory and cache path. The old disposed Entry cannot close that
successor or delete its cache. A shared fake Tailscale-state file stays intact;
this is not a Go Tailscale disposal or custody proof. Rust-authorized generation
ordering, complete worker/network/DNS drain, TS/cache custody and ledger capacity
remain separate gates. Capabilities stay empty and coverage remains 4/11.

Validation of the Host seam follow-up (2026-09-30): targeted suites passed
37 tests; the full Kotlin XML receipts contain 120 tests in 13 suites with no
failures, errors or skipped tests. The completed Gradle PTY handle was lost during
daemon recovery, so no duplicate test run was made. Bridge checking passed
31 commands; the updated six Rust source-contract tests passed with the real
std-only source-probe library and standalone `rustc --test`. This does not claim
a full Cargo/runtime or Android device test. `git diff --check` passed; no APK
or Go implementation change was made.
