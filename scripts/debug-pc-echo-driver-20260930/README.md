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

Pure checks: `POLARIS_NO_KERNEL_RUN=1 PYTHONDONTWRITEBYTECODE=1 python3 -B tests/test_driver.py`.
Synthetic fixtures only test logic and cannot construct runtime evidence or issuer custody.
