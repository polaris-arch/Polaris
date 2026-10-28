# Private Android core echo root adapter

Status: `ImplementedAwaitingUnifiedValidation`. Import and pure tests have no network,
child, kernel or device effects. Runtime execution belongs to the later unified USB/LAN
window; `POLARIS_NO_KERNEL_RUN=1` rejects the run entry before resources are acquired.

This adapter invokes the existing PC `controller_run` on the main thread and retains its
original Child/wait/receiver custody. It loads the exact retained `lan.py` bytes only after
matching the frozen `executionSourceManifest`, passes both execution source originals to
the existing child loader, and verifies the canonical operator plan's source binding. The
PC author owns those five files; this directory supplies the missing root caller.

Inputs are private operator plan/session-secret files, the final PC source receipt, the
Android source receipt with its actual `commit` and independent `packageArtifact` record,
the external candidate APK, one exact USB
serial and a new private evidence directory. The Android source receipt's exact SHA is the
existing batch `expectedSourcePin`. The external APK and actual package-manager-selected
installed base APK are read and compared before the receiver starts. Split packages are
unsupported. No `approved`/`installed` boolean, copied Ready file or bare source SHA is a
runtime issuer. The driver keeps the actual installed bytes, source bytes and original
controller stream privately; it prints no nonce, key or input on failure.

The final build producer's `packageArtifact` has exactly `sha256`, integer `bytes`,
`packageName: "com.polaris2.app.debug"` and `variant: "Debug"`; actual installed/external
bytes must match that original record and the source commit in the same receipt. This
adapter consumes that independent build original; it does not synthesize an installation
or source-to-package assertion from Android settings. Before an actual build/installation
and current original receiver exist, runtime admission is unavailable.

The Android private plugin writes a session-key authenticated one-shot challenge under
`files/debug-pc-echo-v1`. This driver reads it through fixed `adb run-as` verbs, requests the
one original `readyHandoff(requestId)` on its controller pipe and authenticates the reply
to that same session. The driver deducts its entire pre-request roundtrip from the remote
remaining lease; Android additionally uses its own pre-request elapsed anchor. Both retain
the original PC deadline, global32 budget, original receiver/socket IDs and original Ready
raw framing. Neither machine compares remote numeric clocks or starts a fresh 120s timer.

After Ready, the one fixed matrix requests 24 original Counters snapshots (four per group),
with exactly one outstanding original control request and ordered private phase IDs. TCP
and UDP positive/wrong-nonce/wrong-port groups use the actual core ingress; wrong-port is
rejected before outbound. Foreign-peer negatives remain `Unsupported`/`Incomplete` until
the unified window supplies an independently authorized actual actor. No direct app UDP
to the PC is implemented. Public witnesses retain exact sent/returned counts and hashes,
and `rootNativeWitness: Unknown`; echo alone does not prove SDK network, UID, native
quiescence, policy, managed readiness or release eligibility.

The future run command is `python3 driver.py --serial SERIAL --pc-plan PRIVATE_PLAN
--pc-source-receipt FINAL_PC_RECEIPT --android-source-receipt FINAL_ANDROID_RECEIPT
--session-secret PRIVATE_SESSION_KEY --apk EXTERNAL_APK --evidence NEW_PRIVATE_DIRECTORY`.
Do not run it during source preparation. One run owns one original controller and one
Android batch; errors stop that original controller and do not retry or reissue Ready.

Pure checks: `POLARIS_NO_KERNEL_RUN=1 PYTHONDONTWRITEBYTECODE=1 python3 -B tests/test-driver.py`.
Synthetic fixtures only test logic and cannot construct runtime evidence or issuer custody.

The optional `--pc-local-foreign` profile is implemented for later review and a separately
authorized execution window. It is off by default and cannot bypass the before-effects
`POLARIS_NO_KERNEL_RUN=1` gate. It uses the same ordered private worker, original controller,
receiver, socket IDs, Condition and sole original Wait. At the existing TCP/UDP foreign
negative phases (private sequence 7/19), the worker retains the original B snapshot, binds
one IPv4 socket to the exact approved PC address with an OS-selected port, and checks the
actual source and connected tuple. The source must differ from the expected Android sender.
It sends the original bounded nonce frame to that protocol's approved port once; TCP may
complete partial writes, but UDP never resends a partial datagram. Socket/pipe IO and close
run outside the record/custody locks.

Only the original receiver's adjacent Request with the same source, receiver/socket identity,
complete frame count/hash, `foreignPeer` classification and no echo, followed by the next
phase's original C snapshot (sequence 8/20), can produce
`ObservedPcLocalSourceRejection`. The exact counter change is one total and one foreignPeer;
every other category/protocol stays unchanged. Timeout alone is insufficient. The actual
original Handoff must have at least 16 requests remaining; global32, 24 snapshots, 25 private
requests and Android's current 18 attempt/14 potential send contract remain unchanged.
The phase deadline starts before requesting B and is capped by the original Ready expiry;
the same absolute deadline continues through Request and C (sequence 8/20 does not start a
new two-second window). Individual socket operations use at most its remaining time or one
second. No new lease is issued. Before acquisition, socket IO and the normal reply, the
worker reads the current authenticated request outside locks, then rechecks Stop, Closed,
the same raw and original phase identity after that blocking read. Cleanup/join and the
normal reply/observation boundary also recheck the same deadline. Reply's postcheck uses
only original identity/status/time, so a legitimate next Android phase is not mistaken
for replacement. Default ordinary mode gains the Stop postcheck without this enabled
phase deadline.

Private `pc-local-foreign.ndjson` contains at most two observations with raw B/Request/C and
their hashes, actual tuples, confirmed byte progress and close/join dispositions. The actual
driver bytes are retained in `original-driver.py`; the existing two-entry execution source
manifest is unchanged. Before a write attempt, `NotSent` means no attempted write. If a send
raises without a returned count, progress is `Unknown` with the last confirmed prefix;
partial success does not certify the final transmitted length. Cancellation, missing or
drifting originals, extra counters, incomplete writes, uncertain close/join or failed
sidecar writes leave `Unknown`, retain partial originals/custody in the existing failure
context and stop the original controller. A failed sidecar write is not retried. A reply
started while current may return after expiry/Stop: its already performed write is retained,
but it cannot commit an Observed result or start the next actor. No completed IO is
retroactively reported as NotSent or removed from the original evidence.

This profile observes a **PC-local IPv4 source-address rejection**, not an independent remote
hardware peer, Android core path or a runtime/device verdict. Original Android ForeignPeer
`Unsupported`/`Incomplete` and `rootNativeWitness: Unknown` stay intact; the sidecar issues no
native, NoOwner, managed or release grant. IPv6, remote foreign actors, the Android WrongPort
tuple correction and the remaining U4/U5 trusted observers are separate pending work. The
34 pure checks run the production worker seam and original receiver codecs with injected
clock/socket/thread/control effects only; they do not run the controller, child, ADB, echo
server, network, app or device.
