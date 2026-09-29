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
| `main.system` | `BoxService.onStartCommand` for Boot, always-on, system relaunch, and duplicate foreground intents | same main owner proof | a rejected no-owner foreground service stops itself; late SystemStart cannot cross seal |
| `main.close` | exact attempt close after Stop/onDestroy/onRevoke | native `CloseService` and `Close` succeeded, then registry released the same attempt, then `ClosedExact` | stop timeout or failed close stays unsettled/Unknown; no generic Stop implies exact release |
| `main.reload` | Service reload callback, including targetless callback | control operation leaves JNI; same main ticket stays owned | retired endpoint declines before native reload; late callback cannot start after seal |
| `login.start` | reserve before login worker queue, state-directory claim, validation, native factory/start | exact login owner close | cancelled or failed factory cannot be silently removed; stale worker cannot enter native after seal |
| `login.close` | explicit close, Rust Drop, serviceStop, 300 s expiry, retry | `CloseService`, `Close`, network and cache cleanup on the same entry | close-before-start tombstones the ID; timeout/late callback keeps the ticket observable |
| `speedtest.start` | reserve before pure config-validation thread and session queue | exact speedtest owner close | its epoch:sequence ID cannot be replayed; cancelled queue cannot enter native |
| `speedtest.close` | explicit close, serviceStop, expiry, main-start preemption | engine native close plus network cleanup on same entry | 8 s timeout is cleanupUnknown, not `ClosedExact`; late close may settle only exact ticket |
| `validation.checkConfig` | reserve before plugin validation thread; also every login/speedtest validation call | `CancelledBeforeBirth`; otherwise `ValidationCleanupUnknown` until Go returns an observable `box.Close` result | Rust timeout/detached callback does not end the Kotlin JNI operation |
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
No partial implementation may clear `historicUnknown`,
Tailscale custody, or enable managed routing.
