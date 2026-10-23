# Real final Linux package acceptance

This is the Linux-only, ordinary-user leg of `package.yml`. It executes the
**already finalized deb and AppImage**, never a rebuilt App, a libtest binary,
a Vite browser, or a replacement core. It is not installation/uninstallation,
FUSE, privileged helper, TUN, Windows or macOS acceptance. A separate TUN leg
must establish its own authorization, routing isolation and recovery evidence.

## Admission and custody

- Normal full-weight `v…` tag packaging is gated automatically. A reviewed branch
  can opt into the actual `package.yml` **workflow_dispatch** inputs
  `linux_package_acceptance=true`, `linux_acceptance_candidate=<exact App SHA>`,
  and platform `linux` or `all`. The actual branch ref, checkout HEAD, committed
  contract, GitHub SHA/repository/caller/run/attempt must agree. Branch runs only
  produce artifacts; they never publish a release. A later tag build must test
  its own newly produced bytes again.
- The setup job and consumer reuse the original `formalGateContext`. Diagnostic
  native-validation/legacy flags (including an explicitly present `0`), light
  builds, alternate core candidates, bundle overrides and skipped quality gates
  are not admission. The existing package producer's legacy `0` is retained,
  not silently unset: its freeze is only a byte record bound to the separately
  admitted setup context, never a formal acceptance receipt.
- Freeze occurs after final Linux hardening/repacking/naming. It records both
  carrier hashes/sizes and extracted App ELF, core and Cronet members, source
  tree/contract/run/attempt. The successful original carrier artifact's actual
  ID and ZIP digest are sealed into a current-attempt index. The consumer checks
  the real GitHub API run/artifact metadata, ZIP digest and extracted byte hashes.
  It also runs the unchanged formal fetch-core comparison and final-package
  native receipt checks. No legacy receipt or formal pin is rewritten.
- Both draft creation and final publication require the direct acceptance job's
  exact `success`, exact accepted artifact ID/digest, and the original Linux
  carrier hashes. Immediately before publication, the actual draft's uploaded
  Linux asset sizes and SHA-256 digests must match the same freeze. Missing,
  skipped, cancelled, stale-attempt, changed-byte or unknown-cleanup results fail
  closed. A consumer-only rerun cannot reuse an earlier attempt: start a new
  full run. Existing immutable carrier artifact naming is not overwritten.

## Actual execution

Each carrier is extracted separately as an ordinary user. deb keeps its
`usr/bin` + `usr/lib/Polaris` relationship; AppImage uses its original `AppRun`
and hooks. A private empty HOME/XDG tree, DBus and Xvfb are used. Official
`tauri-driver 2.0.5` drives the locked release WebView through process-local
`TAURI_WEBVIEW_AUTOMATION=true`; WebKitWebDriver incompatibility fails the gate.
No sandbox, graphics, AppArmor, user-namespace or sysctl escape is installed.
No GitHub token, runner credential, proxy override or loader override is inherited
by the App or its descendants.

First observe the empty-HOME default `singboxDashboard=true`; never prewrite it.
`config_save` with optimistic versioning is used only to supply synthetic private
fixture candidates. Selection, ON/OFF, manual disconnect preference, Auto,
Apply, retry, restart decision and quit use real native WebDriver input. The
bounded observer uses the original Tauri callback Map, leaves immutable invoke
and IPC functions untouched, and forwards each original callback unchanged.
Replies have no command metadata: runtime-result shapes are correlated with the
real click cursor and original transaction status, not labeled as known commands. It does not fabricate responses. Unknown
Apply is never automatically resubmitted; known unconfirmed transactions are
queried by their original status identity.

Both carriers must demonstrate:

1. Healthy loopback A/B selection and ON/OFF selection persistence.
2. Manual switching honoring the disconnect preference; native Auto changing new
   connections while an existing TCP stream survives its real five-minute cycle.
3. Real GUI HOT Apply preserving the core PID and old stream while new TCP/DNS
   uses B, with observed TTL zero. A private exact-file trailing-newline conflict
   proves durability-pending, read-only status, and same-transaction retry.
4. A real rejected missing-interface candidate and declined structural restart
   preserve the old graph, identity and connections; no system interface is changed.
5. Same-HOME restart restores accepted desired state, before an explicitly separate
   small authenticated Rust sideclient checks all six RPCs, missing/wrong secret,
   no-op, replay/status/retry and rejected CLI regeneration Ack. Secrets/config
   enter that client only through stdin, not argv/logs.
6. Native Ctrl+Q reaches the original `tray_quit` path, creates a fresh original
   clean-exit commit marker, and leaves no owned App/core/WebKit descendants.
   PID + starttime + observed ancestry and Linux pidfds are required for any cleanup signal.
   Forced product shutdown, a forced tool kill, an unobserved adopted process or
   leftovers fail; successful cleanup cannot relabel an abnormal exit as normal.

Fixture data traffic is loopback. Normal read-only cold-start IP/update/background
Internet traffic remains allowed and is recorded with bounded owned-socket
snapshots (not packet capture, not proof of zero egress). No user subscription,
configuration or credentials are used. SOCKS traffic may not map Cronet; that is
reported honestly, not asserted to be a Cronet request test. The original final
native proof separately must demonstrate the exact packaged Cronet CAPI engine
initialization and actual-core library mapping.

## Evidence and limitations

Actual App-generated `singbox-runtime.json` is read twice with stable inode,
metadata and owned process identity; its original bytes (not reserialized JSON)
are retained with base64 in a private `0600` runner file. Public evidence contains
only hash/size/PID/read times and bounded non-secret observations. Raw runtime
JSON, HOME, random API secret and raw App stdout are **never uploaded**. Private
originals disappear with the ephemeral workspace; this is not cross-VM original
JSON archival. Screenshots show the native App, not a browser mock.

For this release, actual acceptance remains pending until the genuine 12.2 formal
contract and final App 1.0.0 carriers exist. The generic gate follows the real
committed formal pin; running against the unchanged historical pin would not
constitute 12.2 acceptance.

Source/contract tests and compilation are not a successful package run. A
successful receipt is produced only after both actual carriers complete every
mandatory case and normal cleanup. This code does not imply that a newer kernel
has been formally bound or that any not-yet-produced package has already passed.

## Source tests

```sh
node --test scripts/package-acceptance.test.mjs
python3 -B -m unittest discover -s scripts/package-acceptance -p 'test_*.py' -v
cargo test --locked -p polaris-singbox-grpc --example package_runtime_probe
cargo build --locked -p polaris-singbox-grpc --example package_runtime_probe
```

The Python/Node suites cover only harness primitives, source contracts and
negative custody mutations; mocked unit responses are never runtime acceptance
receipts. The future dedicated GitHub VM job installs only automation/toolchain
prerequisites and runs the actual packages with default production behavior.

The callback observer is deliberately matched to the immutable properties and
public callback Map in [Tauri 2.11.5 core.js](https://github.com/tauri-apps/tauri/blob/tauri-v2.11.5/crates/tauri/scripts/core.js); a changed or non-extensible bridge fails closed.
