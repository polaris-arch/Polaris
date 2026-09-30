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
| `main.bridge` | Plugin Start before foreground-service dispatch; one ticket continues from `VpnBridge` pending into `MainKernelAttempt` | `CancelledBeforeBirth`; born main owners currently remain `Unknown` | Rust bridge timeout detaches but Kotlin ticket remains; late Service attempt must meet sealed admission |
| `main.system` | `BoxService.onStartCommand` for Boot, always-on, system relaunch, and duplicate foreground intents | same main owner proof | fence, request replacement, or occupied registry rejects an ownerless Service; `stopSelfResult(startId)` tears down only that Android request |
| `main.close` | exact attempt operational close after Stop/onDestroy/onRevoke | capability absent: born main owners remain `Unknown` until global DNS drain and native lease release are proved; genuinely unborn reservations may cancel | successful native Close and exact registry release preserve ordinary recovery but do not prove resource disposal; construction failure/revoked completion remains sticky `Unknown` |
| `main.reload` | captured AttemptHandler callback reserves an independent TargetlessReload operation before target capture/config I/O | real callback work returns with the exact target still current, then control `Completed`; main owner remains owned and born-main release proof stays `Unknown` | pure preflight/birthAllowed precede Host work; first actual JNI rechecks entry; construction/revoked completion stays owner+operation `Unknown`; capability not declared yet |
| `login.start` | owner reserve at invocation, independent validation reserve before worker queue, owner birth before setup/JNI | capability absent; born owners remain `Unknown` after ordinary cleanup | consumed ID cannot replay; cancelled/sealed worker cannot enter native; construction failure/revoked return is sticky `Unknown` under ownership lock |
| `login.close` | explicit close and Rust Drop use original ID; serviceStop, 300 s expiry, retry and main preemption retain exact Entry | worker exit, native close, network/DNS and original cache/TS ownership proof remain pending | close-before-start tombstones ID; successful map removal is operational cleanup only and cannot prove TS release |
| `speedtest.start` | ticket installed before pure config-validation thread and session queue; `enterBirth` before prepare's first resource | exact speedtest owner close; capability still absent pending resolver drain | its epoch:sequence ID cannot be replayed; cancelled/sealed queue cannot enter native; native construction failure or revoked completion stays ledger `Unknown` |
| `speedtest.close` | same ticket for explicit close, serviceStop, expiry, main-start preemption | native close, original prepare/Start worker exit, network notifications/thread cleanup, and unused resolver shutdown | 8 s timeout leaves owner observable; used DNS resolver stays ledger `Unknown`; operational Close success preserves ordinary retry/main admission |
| `validation.checkConfig` | common enqueue adapter reserves before plugin thread, login worker queue, and speedtest validation thread/session queue | `CancelledBeforeBirth`; otherwise `ValidationCleanupUnknown` until Go returns an observable construction/disposal proof | failed queue dispatch cancels before birth; Rust timeout/detached callback does not end the Kotlin JNI operation |
| `control.targetlessStop` | default unbound callback reserves before any dispatch; it never resolves the current main owner | no JNI work: `CancelledBeforeBirth`; rejected reservation does nothing; capability not declared yet | old or ambiguous callbacks cannot stop a successor; captured known-owner Stop keeps its existing operational cleanup after seal/capacity closure |
| `control.targetlessReload` | default unbound callback reserves and declines without resolving an owner | no JNI work: `CancelledBeforeBirth`; rejected reservation does nothing; capability not declared yet | a missing target is never repaired by selecting a newer owner; captured reload has its own operation ticket and actual-boundary admission |

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

Current wiring remains **3/11**: `main.bridge`, `main.system`, and
`validation.checkConfig`. The prior `main.close` declaration is withdrawn: main
cores share a process-global resolver, and ordinary Close does not prove all
Go native leases or DNS work have ended. Speedtest and Login owner tickets are installed, but
their capabilities remain empty until their remaining cleanup gates are proved. Other
rows describe required contracts. Deterministic JVM seam tests cover both Stop/construction
failure orders, revoked JNI completion, old-A/new-B settlement, prequeue seal
and queue failure at all three validation ingress paths, request replacement
before/after admission, and exact-startId teardown versus a successor request.
The Android startId comparison is documented by [Service.stopSelfResult](https://developer.android.com/reference/android/app/Service#stopSelfResult(int));
this change has no APK or device validation.
No partial implementation may clear `historicUnknown`,
Tailscale custody, or enable managed routing.

Main-close proof withdrawal changes only the native admission ledger and its
capability declaration. Successful operational Close still releases the exact
registry owner and preserves ordinary reconnect behavior. The target-local
`MainKernelAttemptLedger.lastReleased`/`exactStatus` `AlreadyGone` fact and
`PolarisVpnPlugin.stopMainCoreExact` `Closed` mapping are unchanged. They describe
that operational target, not process-global DNS drainage or Go lease release.
The later global NoOwner coordinator must never substitute these wire results
for those outstanding proofs. An entered but unpublished main owner also stays
`Unknown`; only a reservation that genuinely never entered native may cancel.

Main-close proof withdrawal validation (2026-09-30): full Android Kotlin
compilation and `:app:testUniversalDebugUnitTest` passed 169 tests in 15 suites,
including 11 main-admission and 16 control-contract seam tests, with no failures,
errors or skips. Bridge checking passed 31 commands. CI impact classification
selects Android with no unregistered scopes; `git diff --check` passed.
No operational Service/registry, Rust, Go, Host, SDK or APK change is included.

## Main control contract (production wiring pending final review)

The current primary factory supplies `AttemptHandler(attempt, this)` to
`newStrictCommandServer`. Go's `ManagedService.StopService/ReloadService` calls
the same server's `platformHandler`, which forwards to that captured handler.
Its Stop therefore names the original attempt. The default
`BoxService.serviceStop/serviceReload` methods now reserve and decline unbound
callbacks without looking up the current owner. The captured reload delegates
to the real pure `AndroidNativeMainControls` producer with Service/Host/JNI
adapters. Each invocation uses a separate control operation; no control
capability is declared by this slice pending its final review.

`AndroidNativeControlOperation` enforces the reviewed control contract:

* An exact reload reserves an independent `TargetlessReload` operation ticket
  when its callback arrives, before any queue or config I/O. This kind represents
  callback work for both `main.reload` and `control.targetlessReload`; it does
  not replace the main owner ticket or prove release of a core.
* The captured attempt and server must still be the exact Started owner. The
  operation holds that attempt's existing `operationLock`; Stop revokes it
  immediately and its close worker joins the same lock. Each JNI boundary checks
  the exact target again. `enterBirth` occurs before the callback's first JNI,
  including `OverrideOptions` allocation and `setError`, and rejects a
  reservation sealed or capacity-closed while queued. A pure `birthAllowed`
  preflight precedes the Host adapters. Their existing cleanup and claim order
  is retained outside the construction boundary: Speedtest preemption, Login
  config claims/preemption, legacy-marker check, then the actual main JNI call.
  Existing transient Close belongs to its original owner ticket; it is not a
  new main construction. If seal races after Host preflight, the final
  `enterBirth` still blocks main JNI and error-report JNI.
* Pure configuration preflight and the dual-mode reload tombstone run before
  native construction. The ordinary reconnect notice keeps the live core and
  remains tied to the original attempt/notice owner. Hot mode switching still
  uses its existing management API and is not converted into reload or restart.
* A normal config-load/preflight error may be published through `setError` only
  after this same operation enters admission and the original target is still
  current. If seal, capacity, or revocation forbids that JNI, retain the original
  error for fixed-label logging; never route it to a successor. No new UI error
  surface is proposed. Existing safe load errors remain visible through native
  `setError` while admission and exact ownership permit it.
* Only reaching the real main construction boundary sets `constructionEntered`.
  A construction throw, revoked return, or later Host/target completion failure
  after that boundary records main-owner construction `Unknown` under
  `operationLock`, and records operation `Unknown` before attempting `setError`.
  Pure Host/config failure before construction does not poison the main owner.
  Error reporting uses the same entered operation;
  its success cannot clear `Unknown`, and its failure cannot replace the first
  error. Ordinary error-report failure does not assert construction uncertainty
  for a core that was never reloaded. The first Throwable is retained for
  fixed-label logging, including when error-report JNI or a final revocation
  check fails; sensitive raw messages and stack traces are not newly logged.
* Only the real synchronous callback/native work returning with its exact target
  still current can complete an entered operation. Pure refusal cancels before
  birth. A stalled callback remains captured and nonterminal; a Stop timeout
  cannot complete it, remove it, or release its owner. Its later revoked return
  remains `Unknown`, even if exact operational Close subsequently succeeds.
* Default targetless callbacks have no trustworthy owner identity and must
  decline without resolving the current owner. A successful reservation is
  cancelled before birth; rejected admission performs no JNI. The production
  captured Stop, user Stop, revoke, and destroy still close their existing exact
  owner without reserving a new operation or entering new birth. Seal and
  capacity closure must not block that cleanup.
* Terminal main-close failure no longer invokes `commandServer.setError` from
  `onAttemptClosed`. It retains fixed diagnostic logging, `finishStop`, and
  ledger `Unknown`, without adding JNI after the close attempt.

The seam tests exercise the actual helper with fake JNI latches and real
`MainKernelAttempt`/ledger state, including queued seal/capacity rejection,
first-error preservation, original owner revocation, and late error/notice
delivery. The real production producer tests also pause pure preflight and Host
completion, exercise both native-failure/Stop orders, and deliver an old queued
callback after a real registry release admits a successor. Its returned
operation ticket is internal identity for state inspection, not a completion
or resource-release assertion. Production wiring awaits final review; declared
coverage remains **3/11**. Neither this producer nor its helper emits global
`NoOwner`, a Go native lease proof, or a Tailscale custody receipt.

Production control wiring validation (2026-09-30): actual Android Kotlin
compilation and the targeted producer/control/main/dual-mode suites passed
47 tests in four suites. Full `:app:testUniversalDebugUnitTest` passed 192 tests
in 16 suites with zero failures, errors or skips, including 16 production
control cases and 19 Host lifecycle cases from the combined base. The 31-command
bridge check and six existing Rust transient-login source-contract tests passed;
the latter used standalone `rustc --test` with the real std-only source-probe
library, not a Cargo/runtime build. CI impact classification selects only
Android and reports no unregistered scopes; `git diff --check` passed.
No Host/Sessions/MainKernelAttempt, Go/Rust/UI/SDK, AAR/APK, device or real-network
change/test is included. This slice adds no capability; coverage remains **3/11**.

Control-contract seam validation (2026-09-30): the first targeted run passed
14 tests after compiling the actual Android Kotlin sources. After adding two
first-error/target-check cases, full `:app:testUniversalDebugUnitTest` passed
167 tests in 15 suites, including all 16 control seam cases; failures, errors,
and skipped tests were zero. `node scripts/check-android-bridge.mjs` passed all
31 commands and `git diff --check` passed. `BoxService.kt` and
`AndroidNativeMain.kt` are unchanged in this contract-only slice. These receipts
do not validate a production reload adapter or device callback lifecycle.

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

The strict speedtest network path fences and joins notification callbacks, and
the instance's HandlerThread must exit before an unused resolver may contribute
local cleanup proof. DNS SDK callbacks now retain only pure Kotlin mailboxes
and publish copied results; they never deliver through an ExchangeContext.
Only the original exchange/lookup stack can acquire a one-shot delivery permit
under the resolver metadata gate and call JNI before returning. Seal revokes
Pending/Ready results, rejects new queries and permits, and wakes original
waiters. A previously acquired JNI permit remains counted until its action
really returns or throws; timeout and interruption do not fabricate completion.
An interrupt while waiting for the permit monitor is rechecked before delivery.
Queried cleanup remains `Unknown`. This supplemental proof result does not turn an operationally
successful close into permanent active ownership: ordinary subsequent speedtest
and main Start retain their prior admission behavior.

**Pending P1 gate: queried resolver callback drain and disposal.** A future
independent slice must prove SDK/native query and cancellation drain, original
JNI return, Go cancellation-registration removal and physical JNI proxy disposal.
Local cancellation revokes and wakes first, then queues best-effort
CancellationSignal.cancel; cancellation tasks themselves remain counted until
they really finish. The queried SDK executor continues accepting and running
late SDK tasks because those Runnables also read results and clean up framework
fds. Shutdown, dropping tasks, or rejecting late SDK work is not a drain proof.
API 24–28 synchronous Network.getAllByName remains counted while blocked and
checks the delivery permit only after it returns. Clearing the local hook holder,
callback return, Go context cancellation, or an empty active-session map alone is insufficient.
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
above, and coverage stays 3/11.

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

The process ledger now has a **16,384-record budget**, configured before any
reservation and injectable at a smaller value in tests. This is a retained
metadata entry budget, **not an exact byte commitment**: the count is
`entries.size + usedOwners.size`. An owner reserves two records (ticket plus
consumed ID), a validation/control operation one, and an unseen close one
tombstone. IDs retain their existing 256-character ASCII bound. The budget
permits thousands of complete start attempts in one process; a transient start
also has independent validation records, so it is not a promise of 16,384
successful logins or speedtest sessions.

Every allocation and the combined count use the same ledger monitor. Filling
the budget, or requesting an allocation that cannot fit, permanently closes new
reservations and **every unentered birth**, including the last Reserved ticket
and workers already queued. An unseen close that cannot record its tombstone
also closes admission permanently. Known-ID retirement and already-entered
owner completion/Close continue. No TTL/LRU, terminal-ticket deletion, consumed
ID reuse or `historicUnknown` reset is permitted. Login checks the reservation
before preempting a shared-directory predecessor and still rechecks at native
birth; a capacity-rejected queue does not dispose an already-entered owner.

Capacity closure is **not** an explicit drain fence or a native terminal fact:
it creates no fence ID, capture membership or receipt. A later explicit seal
remains legal and captures existing Unknown facts normally. Born Login tickets
remain Unknown after operational Close, and queried DNS proof remains a
separate P1 gate. Capability sets remain empty for Login/Speedtest, coverage
remains **3/11**, and managed/NoOwner activation remains disabled.

Only this invocation's explicit capacity rejection carries
`ANDROID_NATIVE_LEDGER_CAPACITY_CLOSED`. Kotlin keeps a typed cause internally;
CheckConfig adds an optional `errorCode`, Main uses the rejection whitelist,
and transient start/unknown close callbacks preserve the code. Rust propagates
checker rejection as a typed error and native start rejection through a typed
`SpawnError.source`, then selects the command's capacity outcome. Default
desktop checker behavior and generic fallbacks remain unchanged. No global
capacity latch or raw-message keyword overrides later native Close/network/TS
authorization failures: a real speedtest cleanup failure keeps CleanupUnknown
priority. UI uses the stable code/reason and five localized short messages
instructing the user to **fully close and restart the app**, without rendering
native diagnostics or adding persistent UI.

The live Login operational map remains limited to eight Entries. Successful
disposal clears native/network/cache references; a captured expiry may retain
its disposed Entry's ID/directory metadata until the timer/worker drains. The
bounded ledger retains ticket/state/ASCII-ID metadata, not Entry, config,
authorization URL, File or cache contents. Epoch admission/rotation still needs
an explicit Rust-authorized protocol; this change does not infer authority from
the external ID string or change that contract.

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
changes were performed, and coverage remains 3/11.

Login Host seam follow-up: the production `TransientLoginHost` delegates to
`TransientLoginHostState`, which owns the actual Entry map, state-directory
claims, predecessor disposal, worker dispatch, close callbacks and timers.
Fake-native tests run that same state machine with controlled queues and latches.
The production adapter retains native CloseService/Close, network close and
cache deletion in their prior order. Both Login and Speedtest now use the same
production close-stage seam: nonblocking beginResolverClose precedes native
CloseService/Close, revokes undelivered DNS results and dispatches cancellation
without waiting for it. Login retains its ordinary requireExactClose=false
network path. Real native/network/cache failures retain their original priority
and cleanup conditions; queried resolver proof uncertainty is separate from
operational success. A native serviceStop may reenter the captured Entry close
without a new cleanup lock or a proof wait. Configuration is cleared from the adapter
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
ordering, complete worker/network/DNS drain and TS/cache custody remain separate
gates. The metadata budget described above closes the unbounded-ledger P2
without proving any resource drain. Capabilities stay empty and coverage remains 3/11.

The fence runs at the actual Engine.close stage entry. Login's existing serial
worker and ownership lock still wait for a constructing Start to return before
that close sequence can run; this slice does not bypass those operational waits.
The shared seam is tested through the production HostState and SpeedtestSessions
with fake native boundaries, controlled SDK-cancel queues and JNI latches. Tests
check fence-before-native-join, reentrant/duplicate old-A close versus successor B,
error priority and cache/reference conditions, and an already Delivering JNI call
that stays counted after an operationally successful close. No APK, framework
network operation, device receipt, queried resolver Exact release, or coverage
promotion is claimed.

Validation of the early resolver fence (2026-09-30): the two targeted Host suites
passed 51 tests; the full Kotlin XML receipts contain 158 tests in 14 suites,
with no failures, errors or skipped tests. Bridge checking passed 31 commands,
and the six existing Rust login source-contract tests passed with the real
std-only source-probe library and standalone rustc --test. git diff --check
passed. These receipts validate the production seam and local mailbox ordering,
not Android SDK drain, native box disposal or device behavior; coverage stays 4/11.

Validation of the Host seam follow-up (2026-09-30): targeted suites passed
37 tests; the full Kotlin XML receipts contain 120 tests in 13 suites with no
failures, errors or skipped tests. The completed Gradle PTY handle was lost during
daemon recovery, so no duplicate test run was made. Bridge checking passed
31 commands; the updated six Rust source-contract tests passed with the real
std-only source-probe library and standalone `rustc --test`. This does not claim
a full Cargo/runtime or Android device test. `git diff --check` passed; no APK
or Go implementation change was made.

Validation of the capacity follow-up (2026-09-30): targeted Kotlin suites passed
65 tests; the final full `:app:testUniversalDebugUnitTest` passed **135 tests in
13 suites**, with zero failures/errors/skips. Production Host fake-native tests
cover combined-count boundaries, failed validation reservation cancelling the
original owner, the final Reserved ticket and existing queues rejected before
factory entry, capacity rejection before shared-directory preemption, entered
owner Close success/failure, unseen-close tombstone exhaustion and concurrent
reserve/retire. Capacity never turns real cleanup failure into an exact fact.

Actual `cargo check -p polaris --lib --locked` and
`cargo check --target aarch64-linux-android -p polaris --lib --locked` both
passed in this slice's independent target. The Android check used installed
stable NDK 27.3.13750724 and versioned API-24 compiler/bindgen target; the
installed NDK 30 beta rejected the dependency's unversioned CMake/bindgen target
before source checking and was not used for the passing receipt. Existing
Android desktop/platform warnings were not promoted to errors. With
`POLARIS_NO_KERNEL_RUN=1`, real library tests passed **11 capacity-filter tests
and 16 Android bridge tests**, including optional-code serde, typed source
downcast/raw-message negative cases, default desktop checker behavior, real
Login/Speedtest check/spawn outcomes and real-close precedence. Six supplemental
Rust source-wiring tests passed; these do not replace the compiled tests.

UI tests passed **119 tests in seven files**, including five-language code
coverage, Login and Speedtest error presentation, short Main toast deduplication
and raw-message rejection. `tsc --noEmit`, the 31-command bridge check and
`git diff --check` passed. Gradle external dependency build directories were
isolated inside this worktree to avoid parallel writes in the Cargo registry.
No APK/device test, generation/SDK change or Go implementation edit was made.
