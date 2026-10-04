---
status: active
updated: 2026-09-30
area: polaris
---

# Linux native harness and shared source provisioning

This source slice does not establish Linux runtime P1 completion. The original eight sing-box patches retain their exact bytes and hashes; patch nine wires route sets and consumer close ordering. The manifest's dependencyPatches remains empty until the L/N/R/I/C source freeze. Android build and cached-receipt verification reject that incomplete production graph. No AAR/APK, desktop core or sing-box process was built/launched by this slice.

## Shared source interface

`scripts/core-source-provision.py` exports:

```python
receipt = provision(manifest_path, source_repo, checkout,
                    module_repos={}, go_binary=None)
verify_checkout(checkout, receipt)
flag = source_linker_flag(receipt)  # -buildid=polaris-source-v1-<fingerprint>
binary_receipt = verify_binary(go_binary, binary, receipt)
```

The source manifest is `scripts/libbox-patches/source-manifest.json` for Android and desktop. The output checkout must be new/empty. Core source is detached at sourceCommit and each core patch is hash-checked and replayed. Each dependency entry has `name`, `module`, `upstreamVersion`, `upstreamCommit`, `sourceURL`, `patchFile`, `patchSha256`, `patchedTree`. Module repositories are explicitly supplied through the API or repeated `--module-source MODULE=REPOSITORY`; the helper does not fetch an unpinned dependency or modify module cache.

Dependency source is exported from its pinned upstream commit into `polaris-dependencies/<name>` without nested repositories, then the hash-locked patch is replayed. The actual Git subtree must equal patchedTree. Only the generated core go.mod gets a visible relative replacement. Offline `go list -m -json <declared patched modules>` must report the declared original version and exact replacement directory. The receipt sets `graphScope=declared-patched-modules`, records the exact `moduleGraphQueries`, and includes that scoped portable module graph and its SHA256. It separately hashes the complete generated main go.mod/go.sum as `mainGoModSha256`/`mainGoSumSha256`, records the generated build tree and binds these values plus provisioner/manifest hashes into the source fingerprint. Unrelated optional module metadata is outside that declared graph scope; this is not an all-module MVS graph assertion. It is written to `<checkout>/.polaris-source-receipt.json` and returned by the API; CLI prints the same JSON. Empty dependency lists have `graphScope=core-source-only` and no module query.

Before and after compilation, verify_checkout rejects staged/working source changes and new compiler inputs. Binaries use the deterministic source fingerprint as their Go build ID. verify_binary checks the actual build ID, linked module version/replacement, complete ELF SHA256 and `go version -m` output. Its default required linked module is sing-tun; declared platform-reachable modules are checked whenever linked. Platform builders must reject an incomplete production dependency manifest before packaging. A receipt with `sourceGraphState=source-only` records core patch replay only.

The Android builder consumes this helper and each libbox.so carries a verified binary provenance entry. Cached AAR verification checks the manifest/provisioner, dependency patches, source fingerprint and native binary receipts. Desktop/Windows source builders are owned by the PC session and consume this same interface; iOS may consume it later on its separate branch.

## Native execution boundary

The Linux harness never launches sing-box. A root guardian runs a precompiled Go test ELF copied from an opened input FD into a private root-owned directory, then checks the copied bytes against the declared SHA256. Both guardian and worker re-execute protected copies of one frozen script with isolated Python and a fixed environment. Receipts bind the harness SHA, copied ELF SHA, profile and argv.

The worker has independent user, network, mount, PID and IPC namespaces, a private proc view, and no inherited host namespace FD. It checks all five identities against the guardian and uses NS_GET_USERNS on the actual net/mount/PID/IPC descriptors to verify ownership by its child user namespace before any network effects. Receipts retain uid/gid mappings, parent-death signal and raw process capability/NNP/seccomp state. A tmpfs closed root exposes a read-only tool/library tree, synthetic /etc and /run, only fixed null/zero device binds and private guard/receipt/temp directories. TUN and random-management device nodes are absent. Host D-Bus/Docker sockets, host source paths and claim directories are absent. Landlock ABI>=3 rejects writes outside the private workspace; capabilities are bounded to NET_ADMIN/NET_RAW/SYS_ADMIN in the verified child user namespace. Seccomp permits setns only with CLONE_NEWNET, and an actual private-FD setns positive control runs after these restrictions. Mount/re-root, unshare, new namespace clone, unfiltered setns and host-handle acquisition are rejected. Any failed isolation step remains Unknown with no initial-root fallback.

The guardian owns a monotonic deadline, a separate child process group, a pidfd, kernel parent-death chain and an actual child wait. SIGTERM/INT executes the same cleanup. A separate root cancellation probe waits the killed guardian and adopted descendants. Private PID1 termination also terminates a child that called setsid. Timeout remains Unknown for native tests.

Read-only host snapshots must succeed before worker admission and after teardown. Comparison retains objects and handles; only known IP lifetime fields with a verified elapsed-time countdown and monotonic nft counter values are normalized, recording every changed path/reason. Renewal, counter reset, unfamiliar clock, snapshot failure or configuration difference remains Unknown. Raw snapshots are retained. The final field `controlledNamespaceLifetimeEnded` covers the controlled tree/mount/FD lifetime; `/proc` reference scanning supplements that evidence and does not establish global NoOwner.

N-only tests use this closed profile. NativeTun/DNS, Docker and fw4 need their own compatible private service/environment fixtures; this slice does not sign them from the N tests.

## Commands and finite evidence

Zero-network probes:

```bash
sudo -n /usr/bin/python3 -I scripts/native-netns-harness.py --guardian --probe clean --deadline 5
sudo -n /usr/bin/python3 -I scripts/native-netns-harness.py --guardian --probe timeout --deadline 2
sudo -n /usr/bin/python3 -I scripts/native-netns-harness.py --cancel-probe term
sudo -n /usr/bin/python3 -I scripts/native-netns-harness.py --cancel-probe kill
```

Historical `f5164d47` harness SHA256 is `a846e41a1941131aa7b385a34353065b9433d56234f0d384850447c532e93267`. Its four zero-network probes reported ProbePass, with exact receipts:

- clean: `/var/tmp/polaris-native-evidence-c443h6qr/receipt.json`
- internal deadline/setsid: `/var/tmp/polaris-native-evidence-pk2bypev/receipt.json`
- guardian TERM: `/var/tmp/polaris-native-cancellation-l6469j7i/receipt.json`
- guardian KILL: `/var/tmp/polaris-native-cancellation-hhq0b6a9/receipt.json`

The finite core review accepted the earlier file/FD and cancellation mechanisms but blocked that initial-userns profile under H4. Those historical receipts retain their original scope and do not certify the child-userns fix.

The H4 candidate has script SHA256 `9603da5ad00632ea3f6c56c3e00d999c8b51b4fc8c1b1f62ca4755e30e4440d6` and profile `nft-only-child-userns-v1`. The same four commands passed with this exact protected-script hash:

- clean: `/var/tmp/polaris-native-evidence-jc2wbquz/receipt.json`, receipt SHA256 `9e1be42ec902b24e2bafd58c83569b68b8c64cc43bd26dbcca6a5b6481bf2271`
- internal deadline/setsid: `/var/tmp/polaris-native-evidence-d60on5tz/receipt.json`, receipt SHA256 `47d6286495efa33176fefc1c7ac2bc3adafc53a6d5e4941e0ef77e7efcf29400`
- guardian TERM: `/var/tmp/polaris-native-cancellation-3or3h6wx/receipt.json`, receipt SHA256 `a48c3ec8364f4607800995836a6f599a9ae33e5d96078c94a6ee87760fbba72f`
- guardian KILL: `/var/tmp/polaris-native-cancellation-yi3rtw9h/receipt.json`, receipt SHA256 `ea83802051fc1b509c9c02e57f21bee30cf75a68b5048419d26ae949486ed677`

Clean and deadline probes have successful before/after host snapshots and verified configuration equality, with only recorded countdown normalization on deadline. TERM keeps its inner Unknown; KILL records guardian exit -9 and actual adopted-child waits. Cancellation receipts certify their controlled cancellation/recovery boundary, without an after-host-snapshot claim. Actual NS_GET_USERNS ownership, uid/gid maps, CapEff/Prm/Bnd `0000000000203000`, zero CapInh/Amb, NNP=1, seccomp=2, private setns success and post-mapping SIGKILL parent-death state appear in the worker evidence. This candidate remains subject to the same finite review; no native network test has run.

These receipts are only the isolation negative/positive controls. Native effects require the finite core review to approve this exact profile and the current corrected N source/ELF first. The later N command binds exact tests:

```bash
sudo -n /usr/bin/python3 -I scripts/native-netns-harness.py --guardian \
  --binary /absolute/reviewed-native.test --sha256 <copied-ELF-SHA256> \
  --expected-test TestOwnedKernelT2 --expected-test TestOwnedKernelT3 --deadline 45
```

The test binary receives its borrowed new namespace at FD3 and `POLARIS_NATIVE_NAMESPACE_FD`, parent identity values, private `POLARIS_NATIVE_GUARD_DIR`/`POLARIS_NATIVE_RECEIPT_DIR`, nonce and `POLARIS_NATIVE_FS_ISOLATED=1`. Test list and actual top-level RUN/PASS sets must match, with no top-level SKIP or missing completion and a pending/complete NDJSON journal per test. Raw journals are copied into guardian evidence, with finite file/output bounds. NativeTestsPass certifies this execution envelope and basic top-level test/journal completeness; a fixed N verifier must separately reject subtest skips and bind the nonce/namespace/ELF plus actual transaction ACK semantics to each T2/T3 assertion. It does not establish whole-backend/runtime P1 completion.

Verification already completed: four adversarial pure harness tests; actual patch/module graph/ELF-build-ID provenance fixture with rejection of new compiler input and hash-tampered patch; sing-box consumer Go race tests. Initial core b609 plus nine-patch replay produced Git tree `637955a2007be5785de21a6d598d36443ae1b702`, with dependency list empty. This is explicitly source-only replay, not patched sing-tun/nft production graph evidence. Actual T0–T5, final producer/dependency freeze and package validation remain pending.

The declared-scope helper fixture also succeeds with deliberately unavailable unrelated optional-module metadata. A real one-dependency source fixture replay at `/home/sway/Code/polaris-g-nft-declared-replay-20260930/.polaris-source-receipt.json` selected nftables `v0.3.0-mod.4` with the explicit `./polaris-dependencies/nftables` replacement and recorded main go.mod/go.sum SHA256. Its fingerprint is `6b71555a561cf1ba21213564b09dbafb626daf176eb1e528184b23e9dbb677ea`. This uses the earlier N source snapshot to verify provisioning only: the N dump-filter review fix and the other production dependency sources are still pending. It is neither final production graph nor kernel functional evidence.
