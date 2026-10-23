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

## Automation tooling and preparation failures

Both Linux lanes install the locked `tauri-driver 2.0.5` crate into a private
runner-temporary root. A same-source/run/attempt/job receipt records the Cargo
installation inventory and actual executable hash, plus the installed Ubuntu
WebKit driver package version, architecture, owner and executable hash. The
shared runtime revalidates those bytes before launching the recorded absolute
paths. Neither driver supports the old `--version` probe; a displayed or
hard-coded version string is not provenance. Product HOME and environment
isolation are unchanged.

An always-run, bounded `scope.json` records selected workflow step outcomes,
including failures before the runtime harness starts. It is diagnostic-only,
never an admission, runtime, cleanup or publication receipt. Missing outcomes
remain unknown, and cleanup is not assessed by this recorder. Checkout failure
or VM termination may prevent recording; original workflow logs remain the
source for those failures. A new source/run must produce and test its own
carrier identity; old successful package builds are not relabelled.

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

## Runtime boundary failures

The TUN helper and selected `core-manifest.json` must be regular files adjacent
to the frozen executable. The separately bundled application source manifest
has a different role; it is not a second selected installation manifest. The
selected manifest still passes the complete installation identity/hash contract.

Process admission and pidfd signal authority remain unchanged. A rejected process
records only bounded birth/UID/capability/namespace diagnostics, never argv,
environment values or raw stderr. Unreadable identity remains unknown. Monitor
stop/join runs even when inventory fails; an unconfirmed stop remains a failed
cleanup, not an empty-process success. Workspace removal requires observed process
exit, no unknown ownership, and no remaining mount beneath the private directory.
Unconfirmed exit, ownership or mount prerequisites prevent removal. Removal
errors remain failed without retry or permission changes; no chmod, unmount or
additional privilege is used to force cleanup. Primary scenario and cleanup errors
are recorded separately. TUN entry progress indicates entry reached, not completion
of a password, helper or network action.

Startup failures retain the controller handle before any child is spawned. Fixed
startup stages distinguish local admission, bootstrap staging, broker readiness
and later typed operations. A pre-ready failure is not proof that sudo never ran.
The bootstrap may remove only its own exclusively created, unchanged fixed files
through the held directory identity; unknown or replaced objects remain failed.
After exec, a failure before a managed broker/ledger exists retains unknown staging
rather than claiming recovery. Ordinary code never signals an unknown root task.

Rejected-process diagnostics include bounded original parent births and confirmed
owned ancestry/roles. Process names are hints, not executable identity; readable
executable facts are birth/stability checked and have an aggregate 64 MiB budget.
The per-file one-second check is cooperative between reads, not a hard OS I/O
deadline. Unreadable or changing facts remain unknown. None of these observations
changes UID admission, signal ownership, or the requirement for normal cleanup.

## Evidence and limitations

Actual App-generated `singbox-runtime.json` is read twice with stable inode,
metadata and owned process identity; its original bytes (not reserialized JSON)
are retained with base64 in a private `0600` runner file. Public evidence contains
only hash/size/PID/read times and bounded non-secret observations. Raw runtime
JSON, HOME, random API secret and raw App stdout are **never uploaded**. Private
originals disappear with the ephemeral workspace; this is not cross-VM original
JSON archival. Screenshots show the native App, not a browser mock.

The genuine alpha.12.2 formal contract and App 1.0.0 candidate carriers are now
bound, but packaged GUI/TUN acceptance is still pending. The gate follows the
committed formal pin; historical source checks or successful carrier builds do
not constitute runtime acceptance of those packages.

Source/contract tests and compilation are not a successful package run. A
successful receipt is produced only after both actual carriers complete every
mandatory case and normal cleanup. This code does not imply that a newer kernel
has been formally bound or that any not-yet-produced package has already passed.

Cargo may hardlink its example executable into the public target path. Before
root preparation, the ordinary harness creates a byte-identical, exclusive
private copy and verifies the source and destination identities and digest. The
root bootstrap still requires an ordinary-owner, single-link regular input and
verifies its digest again; the Cargo output is not modified or executed by this
copy operation. This producer compatibility check is not evidence that a past
root-preparation failure had this cause.

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

### Private session bus acceptance boundary

Both Linux GUI lanes use a real, directly supervised session DBus with a
workspace-local `0700` socket directory, `EXTERNAL` authentication and only the
current UID admitted. Its fixed configuration retains ordinary session message
policy, but imports no host policy, service directories, or automatic activation
services. Bounded real `ListActivatableNames` and `GetId` calls verify the bus;
no portal responses are mocked. The system bus and polkit path, WebKit sandbox,
and product GTK/GDK settings are unchanged. This headless GUI/TUN boundary does
**not** validate desktop portals, file chooser, document export, or screen sharing.
If a tested product operation needs an absent service, its original assertion
still fails; those capabilities are not reported as passed. Tool-custody dpkg
queries are direct registered children with bounded output, timeout and reaping,
not adopted-process name exemptions. On an inventory failure, only the exact
registered ordinary session bus can still receive identity-checked TERM; unknown
processes remain failed and prevent workspace removal.

### Actual API endpoint and read-only authentication preflight

The App allocates its management API endpoint during startup. A fixture's legacy
`controlPort` value is not that endpoint. Acceptance binds the unique loopback
API in the stable App-generated runtime configuration to `status.clashApiPort`,
the same owned core birth and the private fixture secret. The existing native
probe verifies the actual listening socket's ownership. Reconnection must bind
the new endpoint; no fixed port or previous process endpoint is assumed. Public
startup diagnostics contain only fixed stages and bounded non-secret facts,
never the runtime configuration or API secret.

The explicit `linux_auth_preflight_only` Package input runs only the existing
Ubuntu setup job. It requires the exact candidate SHA and excludes all package,
GUI, TUN and credential-window inputs. It reads only the current runner's public
account/group facts and evaluates the existing administrator-group prerequisite.
It does not read shadow/password data, invoke privileged commands, change groups
or polkit policy, build packages, or run the App/core. All other jobs are skipped.
Its JSON is diagnostic evidence, not authentication, recovery, or TUN acceptance.
Passwordless sudo access is not evidence of membership in a polkit administrator
identity; an unmet prerequisite remains a failure, not an automatic exemption.
