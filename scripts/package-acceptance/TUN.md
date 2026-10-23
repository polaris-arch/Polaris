# Same-final-carrier Linux TUN acceptance

This is a separate, explicitly selected leg of `package.yml`. A deb and an
AppImage each run on a **fresh disposable GitHub-hosted Ubuntu VM**. This code
has source/unit/mock coverage; that is **not evidence of a successful native
GUI, helper installation, authentication or TUN run**.

## Exact scope and identity

- Reuse the original package transport, formal source gate, acquisition receipt
  and final-package native proof. No App/core rebuild, repack, fake producer,
  diagnostic-as-formal receipt or replacement core.
- Require the genuine `c6c0d8b3b83ab4d6e7b953dd2dbdb7efe6f8021a` formal
  `v1.15.0-polaris-alpha.12.2` materials. The still-old 814 contract cannot pass.
- Verify the packaged helper against the adjacent original selected-core
  manifest and the actual packaged core/Cronet. App version stays `1.0.0`.
- **deb is extracted with its original usr/bin + usr/lib layout**; AppImage uses
  its original AppRun/hooks. This does not test deb package-manager installation,
  uninstall or FUSE. What is actually installed/uninstalled is the production
  privileged helper, through its real GUI → pkexec path.
- Ordinary-UID GUI, private HOME/XDG/DBus/Xvfb, default graphics and sandbox,
  original empty-HOME dashboard=true. No mock UI, libtest App, sandbox escape,
  AppArmor/userns/sysctl tweak, helper-unit drop-in or alternative helper.
- IPv4 controlled data plane only: one backend namespace/veth; A/B are different
  bound proxy source addresses, not independent physical egress networks. A
  specific synthetic route is not an enterprise VPN acceptance claim.
- Normal cold-start background read-only Internet traffic is allowed and
  observed, not represented as zero egress. Kernel sing-tun `resolvectl` is
  independent of the App's `takeoverSystemDns` preference: temporary system DNS
  and runner connectivity changes are expected possibilities.

## Closed-by-default dispatch authority

The normal Package dispatch leaves `linux_tun_acceptance=false`. To select this
leg, the actual direct `workflow_dispatch` must contain:

```
linux_package_acceptance: true
linux_tun_acceptance: true
linux_acceptance_candidate: <full exact reviewed current App SHA>
platform: linux | all
linux_tun_auth_mode: existing-session
linux_tun_credential_authorization: ''
```

The original repository/caller/ref/source/tree/clean-checkout/run/attempt formal
checks remain mandatory. Both original carriers must exist in this same attempt.
Self-hosted runners, reusable Risk/manual impersonation, lightweight builds,
legacy flags including `0`, old attempts and overridden bundles are rejected.

`existing-session` performs a real noninteractive subject authorization check;
it does not claim an agent or credential exists. Missing usable authorization
fails before product/network mutation. Registration of pkttyagent is **not**
authentication. Both original install and uninstall transactions must separately
prove GUI birth → original pkexec → same-birth root bash, real callback success,
postconditions, and complete process closure.

### Separately approved temporary existing-runner password

**Do not dispatch this mode without the user's separate approval for this exact
credential action. General TUN installation authorization is not that approval.**
No such action was executed while implementing or testing these sources.

Only the authorized dispatcher may select:

```
linux_tun_auth_mode: temporary-existing-runner-password
linux_tun_credential_authorization: AUTHORIZE TEMPORARY RUNNER PASSWORD AND RELOCK FOR THIS RUN
```

The actual dispatch payload must match its actual repository, sender/actor, ref,
full candidate SHA and caller. This mode rejects run attempts other than 1 and
actor-changing reruns: start a newly approved full run instead. Permission binds
that execution, its source tree, one fresh run token, actual observed GUI UID and
unique existing account, the install/uninstall transactions, a bounded expiry,
and at most 90 seconds per transaction. It is permission to perform the action,
**never proof that pkexec authenticated**. No GitHub Environment, new secret or
approval service is created.

The precise potential side effects are:

1. On this single-use VM only, resolve the existing ordinary GUI UID to exactly
   one current login account. Require an existing administrator group and
   `passwd -S` status `L` or `NP`. Never read shadow, overwrite an existing usable
   password (`P`), add accounts/groups, or alter sudo/polkit policy.
2. Freeze controller, launch identity, test client and watchdog code in root-only
   paths. Verify independent root recovery timers before setting a credential.
   Persist a separate setting intent and exact writer birth before sending any
   random password to the fixed writer.
3. Generate a fresh random password in private process memory. Feed only an
   anonymous pipe to `/usr/sbin/chpasswd --crypt-method SHA512 --sha-rounds 100000`.
   This selects this temporary hash operation, not global PAM/password policy.
   No secret enters argv, environment, report, screenshot or log.
4. Register real pkttyagent for the exact GUI PID/starttime subject. Its private
   controlling PTY must offer the same approved identity and have echo disabled
   before input. Product pkexec and normal PAM/polkit authentication remain real.
5. Close each transaction promptly. Stop/reap the tracked credential writer and
   owned authentication descendants before final `/usr/bin/passwd -l <account>`;
   prove final status `L`. Independent recovery does the same even if the mutable
   product ledger is damaged. A missing setting intent does not grant authority
   to lock an account this run did not modify. Unknown/late writers, forced kills,
   missed ownership or unproved re-lock permanently fail the run.
6. Erase private in-memory buffers best effort. Never restore an empty password.
   A **locked temporary hash may remain in shadow until the VM is destroyed**;
   no claim of hash removal or exact original shadow restoration is made.

There is no password setter, arbitrary command/path, generic PID-kill, secret
setter or unbounded network operation in the GUI/controller IPC.

## Execution, observations and termination

All tooling/material downloads occur before the bounded TUN window. The root
controller is QA infrastructure launched with the runner's existing sudo; it
never installs the product helper in place of the GUI. The original production
systemd helper remains untouched by test drop-ins. The ordinary supervisor never
acquires root signaling rights, including a same-PID UID-changing pkexec exec.

The production helper/core must match registered birth, executable bytes,
system namespaces and service cgroup. The core must carry exactly the reviewed
ambient/effective capabilities. Only two separately root-owned **read-only**
probe actions (capabilities/status) inspect the protected core; the original
six-RPC nonroot test client's ownership requirements are unchanged. This TUN leg
does not claim six-RPC TUN negative coverage.

Mandatory cases include actual default graph, captured nonloopback A/B TCP and
UDP/TCP DNS, durable GUI graph Apply with old A stream/new B stream and fresh
TTL0 A→B DNS, ordinary selection, real five-minute Auto preserving its old live
stream, manual preserve/disconnect preference, rejected uncovered target with
restart declined, the synthetic specific route, and normal restoration.
Mature offline tshark reassembly correlates every framed nonce with the actual
ordinary client and backend origin. Simulated analysis is never runtime PASS.

Normal completion requires real GUI OFF, exact core exit, real helper uninstall,
root service/path/cgroup closure, original tray-quit commit marker, both privileged
and ordinary native process reaping, exact owned fixture cleanup, baseline network
restoration, authentication window closure, watchdog cancellation, and new
DNS/verified-HTTPS control connectivity. The product timer is bounded to 600
seconds with a reserved recovery margin; per-transaction credential timers are
shorter. Any forced/unknown cleanup, timeout or cancellation is permanent failure,
even if subsequent best-effort recovery succeeds. Unknown resources are not
blindly deleted. If recovery cannot prove restoration, disposable VM destruction
is the final boundary; it is not relabelled a normal exit.

Runtime JSON original bytes are retained only in private 0600 ephemeral files.
Public evidence contains their SHA/size/PID/read time and sanitized structure,
not the API secret or a claim of cross-VM original-byte archival. Raw pcap,
PTY output, account material, root ledger, private HOME and App stdout are not
uploaded. The original uninstaller's intentional state/lease remnants and frozen
test code expire with the VM.

## Publication and remaining runtime prerequisites

If this run explicitly selects TUN, **both** draft creation and final publication
require success of the whole two-carrier TUN matrix and re-download current-run,
current-attempt artifact IDs/digests to compare the same final carrier bytes.
Failed, skipped, cancelled, blocked or unknown TUN results never publish. If TUN
was not selected, existing baseline publication requirements remain unchanged;
no TUN PASS is asserted. `publicationEligible=false` on the TUN report means that
it cannot independently replace the formal baseline receipt.

Before a real run, the genuine c6 formal carrier/receipt and committed source
must exist; the hosted image's WebKit driver/default sandbox, systemd, TUN,
polkit/PAM, process visibility and tshark must work as observed. There is no
fallback that disables protections. Temporary-password mode additionally requires
that exact execution approval described above. Source/mock passes prove none of
these actual native prerequisites.
