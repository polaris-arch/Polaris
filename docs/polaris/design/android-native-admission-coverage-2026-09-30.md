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
| `login.start` | reserve before login worker queue, state-directory claim, validation, native factory/start | exact login owner close | cancelled or failed factory cannot be silently removed; stale worker cannot enter native after seal |
| `login.close` | explicit close, Rust Drop, serviceStop, 300 s expiry, retry | `CloseService`, `Close`, network and cache cleanup on the same entry | close-before-start tombstones the ID; timeout/late callback keeps the ticket observable |
| `speedtest.start` | ticket installed before pure config-validation thread and session queue; `enterBirth` before prepare's first resource | exact speedtest owner close; capability still absent pending resolver drain | its epoch:sequence ID cannot be replayed; cancelled/sealed queue cannot enter native; native construction failure or revoked completion stays ledger `Unknown` |
| `speedtest.close` | same ticket for explicit close, serviceStop, expiry, main-start preemption | native close, original prepare/Start worker exit, network notifications/thread cleanup, and unused resolver shutdown | 8 s timeout leaves owner observable; used DNS resolver stays ledger `Unknown`; operational Close success preserves ordinary retry/main admission |
| `validation.checkConfig` | common enqueue adapter reserves before plugin thread, login worker queue, and speedtest validation thread/session queue | `CancelledBeforeBirth`; otherwise `ValidationCleanupUnknown` until Go returns an observable construction/disposal proof | failed queue dispatch cancels before birth; Rust timeout/detached callback does not end the Kotlin JNI operation |
| `control.targetlessStop` | old targetless Stop callback before dispatch | control callback leaves its work; main owner remains until exact close | queued/late callback cannot disappear from a sealed receipt |
| `control.targetlessReload` | old targetless reload callback before dispatch | callback returns after any JNI work; main owner remains | reload rejected by seal cannot enter new native work |

Cold-process bootstrap is shared by all rows: only `ENOENT` for
`mesh-route-state.required` opens admission. Marker presence or any other stat
failure keeps the process closed. Seal is permanent for this process; a matching
fence may reread the frozen captured set, while another fence is rejected.

The Go `checkConfig` implementation currently calls `box.New` and ignores its
`Close` return. Many native `Close` methods also return nil for objects that
were never `Start`ed, and partial `box.New` failures do not roll back every
created resource. A validation that entered JNI therefore remains
`ValidationCleanupUnknown`, even after its Java call returned. A future Go
construction/disposal contract must prove both successful and partial-failure
cleanup of unstarted resources; merely exposing `Close() == nil` is insufficient.
Main Start/Reload has the same partial-construction risk. A construction failure
or cancelled completion records `Unknown` while holding the same attempt's
`operationLock`, before Stop's close worker can settle it. A successful ordinary
CommandServer close cannot clear that fact. Exceptional/cancelled preparation,
close, or registry-release futures also remain `Unknown` without throwing from
the settlement callback.

Current wiring remains **4/11**: `main.bridge`, `main.system`, `main.close`, and
`validation.checkConfig`. Speedtest owner tickets are installed, but its
capabilities remain empty until queried resolver cleanup can be proved. Other
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
Go partial-construction disposal remains a separate mandatory proof contract.

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
