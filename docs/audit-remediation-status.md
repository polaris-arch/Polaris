# Reconstructed audit remediation batch

All four implementation review partitions have statically closed their reported findings. The first full GitHub CI batch on App c2226c3c47f0478918b05c2e321f0e221152ab2f reached terminal failure and informed the consolidated fixes below. The amended source snapshot has not yet undergone full product compilation, tests or CI; final integration metadata remains subject to its own review. No historical PASS certifies these reconstructed bytes.

## Original 45 findings

| Finding | Owner | Implementation / validation status |
|---|---|---|
| H-F1 | hot | Rebuilt; partition static review closed; product validation not run |
| BC1 | windows | Rebuilt; partition static review closed; product validation not run |
| N1 | libraries | Rebuilt; partition static review closed; product validation not run |
| PC1 | runtime | Rebuilt; partition static review closed; product validation not run |
| S1 | libraries | Rebuilt; partition static review closed; product validation not run |
| STORE-RC1 | libraries | Rebuilt; partition static review closed; product validation not run |
| STORE-RC2 | libraries | Rebuilt; partition static review closed; product validation not run |
| 04-C1 | runtime | Rebuilt; partition static review closed; product validation not run |
| CE-B1 | libraries | Rebuilt; partition static review closed; product validation not run |
| CE-R1 | libraries | Rebuilt; partition static review closed; product validation not run |
| CE-R2 | libraries | Rebuilt; partition static review closed; product validation not run |
| N2 | libraries | Rebuilt; partition static review closed; product validation not run |
| N3 | libraries | Rebuilt; partition static review closed; product validation not run |
| N4 | libraries | Rebuilt; partition static review closed; product validation not run |
| OB-R1 | libraries | Rebuilt; partition static review closed; product validation not run |
| OB-R2 | libraries | Rebuilt; partition static review closed; product validation not run |
| PH1 | runtime | Rebuilt; partition static review closed; product validation not run |
| RC3 | libraries | Rebuilt; partition static review closed; product validation not run |
| RC4 | libraries | Rebuilt; partition static review closed; product validation not run |
| BC2 | runtime | Rebuilt; partition static review closed; product validation not run |
| PH2 | runtime | Rebuilt; partition static review closed; product validation not run |
| RM05-1 | runtime | Rebuilt; partition static review closed; product validation not run |
| RM05-2 | runtime | Rebuilt; partition static review closed; product validation not run |
| WH1 | windows | Rebuilt; partition static review closed; product validation not run |
| HOT-APP-F1 | hot | Rebuilt; partition static review closed; product validation not run |
| HOT-APP-F2 | hot | Rebuilt; partition static review closed; product validation not run |
| UIP-03 | delivery-ui | Rebuilt; partition static review closed; product validation not run |
| UIP-04 | delivery-ui | Rebuilt; partition static review closed; product validation not run |
| UIP-05 | delivery-ui | Rebuilt; partition static review closed; product validation not run |
| D-01 | delivery-ui | Rebuilt; partition static review closed; product validation not run |
| D-02 | delivery-ui | Rebuilt; partition static review closed; product validation not run |
| D-03 | delivery-ui | Rebuilt; partition static review closed; product validation not run |
| NA-01 | delivery-ui | Rebuilt; partition static review closed; product validation not run |
| NA-02 | delivery-ui | Rebuilt; partition static review closed; product validation not run |
| NA-04 | delivery-ui | Rebuilt; partition static review closed; product validation not run |
| NA-05 | delivery-ui | Rebuilt; partition static review closed; product validation not run |
| BC3 | runtime | Rebuilt; partition static review closed; product validation not run |
| BC4 | runtime | Rebuilt; partition static review closed; product validation not run |
| CUSTODY-RC1 | runtime | Rebuilt; partition static review closed; product validation not run |
| NA-03 | delivery-ui | Rebuilt; partition static review closed; product validation not run |
| PC3 | windows | Rebuilt; partition static review closed; product validation not run |
| PH3 | runtime | Rebuilt; partition static review closed; product validation not run |
| RC5 | libraries | Rebuilt; partition static review closed; product validation not run |
| UIP-01 | delivery-ui | Rebuilt; partition static review closed; product validation not run |
| UIP-02 | delivery-ui | Rebuilt; partition static review closed; product validation not run |

O02 was previously completed and is not reopened or included in these 45.

## Additional required scope

- testing-merge: rebuilt persisted static integrity verified awaiting dual parent commit. Preserve full dfb803 testing ancestry; recover official sing-tun9ffa source baseline, local patch tree and exact two binding JSONs; verify original source archives, not old hashes alone.
- kernel-ci-repair: baseline preserved. Retain all 7baae CI repairs and governance ancestry; final remove only dots-protocol-codegen.yml and dots-baseline-materialize.yml.
- app-main-merge: five real main blobs verified awaiting dual parent commit. Integrate five README files from 51587; final parents82db and51587. Read actual blobs and verify; current compare confirms3 unique main commits, net5README.
- candidate-ci-formatter: source static review closed; candidate SHA rebound once to kernel 68ffb209 repair source; kernel four-workflow validation passed and the same source is merged to polaris-main; App validation remains incomplete. Actual candidate JSON only after true kernel commit; same-repository batch PR-only candidate mode, ordinary PR formal; cheap format and existing native chain preserved; formatter allowlist rederived from rebuilt manifest.
- windows-system-acceptance: source static review closed execution not run. User approved fresh SYSTEM and PrebuildUserDomain paths; wire candidate Windows job to one build; do not claim execution success.
- source-metadata: identities refreshed pending metadata review. Legacy fork-source-fingerprint-v1 collector unchanged/historical-only; current native_release_index→upstream-native receipt/assets→App reviewed-kernel-source-v1/source_policy/upstream-release chain retained. New actual source/asset identity required; never relabel historical receipts.
- durable-integration: owner sources admitted final code only snapshot readback required. Every owner increment first persisted to Library, materialized back and SHA256 checked; only verified packages admitted. Final complete freeze likewise persisted and read back before root remote writes. Two governance branches only; no main/tag writes or product gates.

The first App collection used kernel commit 90becdbed62ea3602ed2744784d574341f754db6 (tree abc204de1a42cf78cd488b40087bbf093fc50f7a), whose Windows failure was known. For the consolidated App rerun, candidate-source.json is now bound once to actual kernel repair commit 68ffb209ded1f9337f9115741cba24f9f4c63421 (tree 780aeae3833770877f7cb386cc1632b778473282); its four-workflow kernel CI has now passed and this same source is merged to polaris-main. The first full App CI batch was run against the earlier 90becdb candidate to expose independent Rust/platform issues without waiting for each kernel repair; its terminal failures and the consolidated repair are recorded below. Windows gates remain required and unchanged. This is failure collection, not acceptance: no PASS, mainline integration, or runtime release is claimed. App reruns will be consolidated after collecting its own failures rather than following every small kernel repair. The prior frozen App A1 tree 829c5f87943d109df53e0290aafde33568a282e8 and kernel K1 commit 918198714a6ef8a7661e0c566761a0c0892fb53a (tree 40e4fbc23ae827f3e001d84610ce95b2cb4a7981) remain stable historical source references; they are not relabeled as current validation evidence. It applies only to the exact same-repository remediation PR or explicit candidate dispatch; ordinary PRs remain formal. Formal release-contract source/assets remain bound to their existing release until actual new release inputs are available. Candidate acceptance is not formal release evidence.

The legacy fork-source-fingerprint-v1 collector remains historical-only and unsupported for the current source. Use the existing scripts/ci/native_release_index.py producer, upstream-native receipts/assets and App polaris-reviewed-kernel-source-v1 source policy chain. Historical receipt bytes and introducedCommit references are unchanged.


## First complete App CI collection and consolidated repair

The App baseline c2226c3c47f0478918b05c2e321f0e221152ab2f (tree 2d6ebb1ddbac678b8eaaab4227b56230bb23d8ac) completed CI run 39011499175, UI run 39011499138, and Release Risk run 39011499199 with failures. Only the early format gate passed in CI. Three platform clippy jobs and cross-target clippy stopped on the unreferenced non-iOS test admission method. All three build jobs found the missing hot-switch rule-file import; Windows also found the directory facade cfg and DRIVE_FIXED import/feature gaps. Windows candidate/SYSTEM acceptance did not execute because compilation failed. The missing upload evidence is downstream of that failure and is not acceptance success or an independent producer test result.

The consolidated repair reuses the exact Stop admission predicate for readiness checks, adds Stop boundary regression tests, restores the production rule-file import, enables the existing Windows directory facade without compiling Unix-only marker helpers in Windows production, and imports the official windows-sys fixed-drive constant with its feature. No directory capability check, DACL rule, fixed-drive restriction, warning gate, or useful test is removed.

UI and Release Risk fixture repairs preserve the actual contracts: explicit cheap-format scheduling/dependencies and cache isolation, checkout versus Node setup boundaries, atomic takeover write registration and lock ownership, reviewed coexistence source compatibility hashes, complete source-first validator fixture dependencies, and per-ABI signature evidence with negative controls. Historical producer/evidence identities remain historical. Focused local checks and exact CI-version Rust formatting are recorded in the incremental validation evidence; these do not establish full product validation.

The final same-batch metadata binding now targets kernel repair 68ffb209ded1f9337f9115741cba24f9f4c63421 (tree 780aeae3833770877f7cb386cc1632b778473282, parent 1139d6d95557a686c77d1f2c1a1fa0601e95c349). This avoids deliberately rerunning acceptance against the already-known failing 90becdbed62ea3602ed2744784d574341f754db6 candidate. At the time of that rebind the new kernel CI was still in progress; its current kernel result is recorded below. App Windows acceptance remains required. The reviewed App business source and fixtures are unchanged by this three-metadata-file adjustment. No App gate bypass, kernel repair duplication, mainline integration, or runtime release is included. A single consolidated App publication and complete GitHub CI batch must verify the amended bytes after independent review. No PASS is claimed.


## Second complete App CI collection and consolidated repair

App 9f35601538d12061088f09e5cb3fc26016bf285f (tree 3f2c4b7934aa63652482708dcb6cd00f9228989d) completed CI 39013026770, UI 39013026762, and Release Risk 39013026832. UI, cheap Rust formatting and source-first policy passed. All three native builds progressed into actual tests, which failed (Linux 14 targets, macOS 15 targets, Windows 14 targets); native clippy and cross-target clippy also failed. The Release Risk classifier reported 642 passes, seven failures and four pre-existing skips. Windows candidate producer/consumer and SYSTEM steps did not execute after the native test failure. None of these missing results is acceptance success.

The second consolidated changes address all collected root causes together. Platform compilation/lint repairs retain cross-target numeric and deadline semantics. Source structure changes restore one consistent production/test ownership and cfg/module graph, preserve each regression and its assertions, and retain deny gates; they do not impose file-length limits or split modules based on line counts. Fixture updates follow actual validator dependencies, release/source identities, strict data-loss protection and native Stop custody contracts, with negative controls retained. Runtime fixes and their focused regressions remain subject to the new native CI batch; static review and formatting are not substitutes for execution.

The original complete classifier entry was run in the cloud using the exact CI Node v26.12.0 and official workspace-extracted Android AAPT2 and SquashFS dependencies. At the seven-fixture candidate checkpoint it executed 654 tests: 649 passed, four existing skips, and one environment failure. The mobile suite's 250 Python tests reported only an AF_UNIX socket-creation EPERM from the cloud sandbox. That test and its assertion were retained, no runtime restriction was bypassed, and the local full gate is not recorded as PASS. Final-byte local results and native-CI requirements are in the incremental validation evidence. No product build was run in the cloud or on the user's computer.

Kernel 68ffb209ded1f9337f9115741cba24f9f4c63421 (tree 780aeae3833770877f7cb386cc1632b778473282) has passed all four kernel workflows, including real Windows tests, and is now merged to polaris-main. The App candidate pin already names that exact commit and remains unchanged. Kernel success does not establish App candidate/SYSTEM acceptance, formal release artifacts, or App mainline eligibility. The App stays on its dedicated remediation branch until its own amended bytes pass the required independent review and GitHub gates.


## Third complete App CI collection and focused repair

App 44c5ba47bff7de5155cc8692023e87ab023401b0 (tree f0b9b3c50165494b56a4ce8e0ce37e23007f6bb5) completed CI 39024505229, UI 39024505211 and Release Risk 39024505245. UI, all three native clippy jobs, cross-target clippy, release classifier, source-first policy, four desktop source producers and four desktop installer jobs passed. The Linux lint job then failed rustdoc on one obsolete documentation link. Native workspace tests reached only one common source-probe path assertion failure on all three OSes, plus a macOS case/Unicode-alias fixture collision. Windows candidate/SYSTEM acceptance did not execute after that prerequisite failure.

Apple and Android mobile source producers used the still-historical 577d4b9a source contract and failed the generated managed-directory API contracts. The reviewed 68ffb209 source and its real CI-generated Objective-C header / Java class declarations expose those required APIs. This batch migrates the source-only mobile producer contract to exact 68ffb209, annotated tag object 50b04bff4d35fe7e4e8f41e24bb0cfd0ab6f5a9d, complete source-policy hashes and Go 1.27.2. Required generated APIs, capability/retirement checks, and readonly boundaries remain enforced. No release or asset identity is fabricated; desktop formal release consumption remains a separate identity handoff.

The focused repairs correct the stale rustdoc description, admit legitimate module-root files while retaining exact secret-reader registration and negative path controls, and isolate independent legal-spelling fixtures so case-folded filesystems do not attempt duplicate directory creation. Existing alias-lock rejection tests remain unchanged. This candidate needs independent review and a new complete GitHub CI batch; static checks and the cloud's runnable contract tests do not establish native or mobile build acceptance.


## Fourth complete App CI collection and boundary repair

App d71238ae7429f143aaa5e81e5a73114b96a8e522 (tree be8bbe2278435d609eceeb555543edc136c662f0) completed CI 39026741918, UI 39026741928 and Release Risk 39026741952. All three ordinary native builds/tests, native clippy/rustdoc, cross-target clippy, UI, Android producer/APK chain and all four desktop installer jobs passed. Windows candidate acceptance stopped before producer/consumer execution because default PowerShell JSON conversion rejected a legitimate empty property name in the full desktop source contract. SYSTEM acceptance remained unexecuted. Both acceptance contract readers now preserve JSON dictionary keys with AsHashtable. The same job had selected Go1.27.0 from the go directive despite the source's exact go1.27.2 toolchain: workflow selection now reads the checked-out toolchain pin explicitly, while the producer retains its exact version check.

Apple passed real generated API and Rust build validation, then its Xcode hook lost the inherited reservation descriptor across the build service boundary. Original artifact12099317902 preserves the complete traceback. The TLS native-tool checks now run in the admitted parent before archive and against the final archived PacketTunnel executable; Xcode hooks only verify/copy pinned inputs. Native symbol/export requirements and fail-closed reservation checks remain enforced. No App/device/TLS live-query acceptance is inferred from static or source evidence. This focused candidate requires independent review and a new complete GitHub CI batch before mainline eligibility.


## Fifth complete App CI collection: Windows fixture token ownership

App e22a1f5a21d4f959aa1504edb472083d7dc90175 (tree fcb28bb0d02c0592a1ba3fb080e4495d935fd3dc) completed the next full batch. UI and the entire Release Risk workflow passed, including the real Apple device build chain, Android APK chain and all four desktop installers. Ordinary three-OS build/tests and lint checks passed. Windows candidate executables built with Go1.27.2 and the three activation regressions passed; the fixed native TestWindows suite then rejected multiple newly created test directories because their owner did not match the token user. This is a real fail-closed owner refusal, not a SYSTEM acceptance result. The log did not print the actual owner/default-owner SIDs, so it cannot establish which SID owned each directory. App interop and the subsequent explicit SYSTEM fresh/prebuilt steps remain unaccepted.

Windows documents that a newly created object's owner defaults from TokenOwner, not the parent directory owner. This batch keeps product owner/ACL rules and the reviewed kernel bytes unchanged. A dedicated acceptance launcher duplicates the process primary token, sets only the private duplicate's default owner to its own TokenUser, reads back identity before starting the suspended fixed worker, then checks the real child token and a freshly created probe directory before invoking the original acceptance script. The original token is opened without adjustment rights and must remain unchanged. Private job cleanup, bounded waiting, exact exit propagation and early/failed-receipt invalidation remain required. The new runtime evidence will record the original/default/child/probe owner identities; static inspection does not substitute for that observation.

The existing same-repository PR-only SYSTEM gate is unchanged. Although manual CI can select windows-2022, it cannot currently execute that restricted SYSTEM gate; this batch uses one ordinary complete PR run. Repeated mobile work follows the existing cumulative PR classifier and is not bypassed by this fix. Independent review and real Windows execution must still pass before App mainline eligibility.

Independent review corrected cleanup evidence for a launch-constructor exception: a missing returned child handle now leaves cleanup unconfirmed. Only successful disposal of a returned child can confirm teardown; receipt revocation and failure propagation remain enforced. Source-mutation controls cover both the constructor-null regression and optimistic initial cleanup state. Actual Windows cleanup remains pending CI.

## Sixth focused batch: preserve the missing native failure diagnostics

App 020a71a94e5879206a862ca5c4ba3796acf5879f (tree 208622ecdd6ef20a6dc2935365567fed0c1c23c8) exposed two diagnostic gaps. The Windows artifact proves that the original primary token default owner is Administrators, the private duplicate was normalized to its TokenUser, and the original token remained unchanged. Creating the acceptance child then failed. The preserved message omitted NativeErrorCode, so it does not establish a missing privilege or a particular Windows error. Native producer/consumer and SYSTEM acceptance remain unexecuted. The macOS arm64 installer compiled its release binary, then DMG bundling failed; its artifact contained only the unchanged official bundler script, without the failing subprocess output.

This batch records the Windows error code and system text at the failing API boundary, preserves wrapped/aggregate error chains separately from cleanup failures, and reads privilege names and flags through query-only token access. It does not enable privileges, switch process-creation APIs, alter the parent token, dump token bytes or credentials, or relax owner/ACL gates. macOS bundling retains the same command and arguments but enables official CLI debug output and preserves its original stdout/stderr even on failure. There is no retry of a failed bundler or successful replacement result.

The diagnostic commit is intentionally eligible for [skip ci] followed by existing targeted dispatches: CI windows-2022 in exact kernel candidate mode, and Package macos-arm64 on the dedicated branch. No selection or SYSTEM authorization rule changes. Windows dispatch cannot exercise the PR-only SYSTEM steps. Package still builds its required four-core inputs and core wire checks, uses the existing full release build configuration, and cannot publish from this non-tag single-platform run. Automatic required checks remain pending; these diagnostic runs cannot establish merge eligibility. After the actual errors are identified and repaired, complete necessary PR gates are still required before any mainline decision.

## Public native archive name compatibility

The upstream native producer now distinguishes its public Release basename with archive.releaseName. The App accepts only the exact sing-box-polaris- prefix substitution of an already validated original sing-box- archive name. A legacy index with no field still uses its original name; explicit null, empty, mistyped or divergent values do not fall back. No publicName alias is accepted. The original archive.name/root, row.id, member inventory and byte/hash pins remain unchanged. Public releaseAsset name/URL, Release ID, asset ID, bytes and SHA remain bound through the existing strict consumer and full index-to-contract receipt comparison. Additional indexed recipes with explicit release names obey the same mapping.

This is compatibility code only. It does not invent or update a Release/asset identity, rename internal APIs, change a candidate source pin or publish an asset. The upstream contract regression suite is now included by the existing root consumer test entrypoint. Newly branded assets have not yet been downloaded in production; real public identity readback and the independent review remain required before adopting a new release contract. The separate Windows/macOS diagnostic work remains the priority and is not changed by this isolated candidate.

The same upstream native payload path also assumed the historical sourceFingerprint.goVersion field. It now selects the field by the already supported receipt schema and requires equality with the existing exact release toolchain policy. An upstream missing top-level goVersion cannot fall back to a fabricated legacy fingerprint, and a historical row cannot borrow the upstream field. Real upstream and legacy shape fixtures cover both successful cases and reject mixed/missing/mismatched shapes.


### Candidate after caaa9082 targeted diagnostics

The targeted Windows run 39036173488 reached 3,101 passing App library tests, one failing custody phase watchdog and 18 ignored tests; it did not reach the private-token launcher, so it supplies no new Win32 launch code. The timeout test now waits for the controlled child to enter its pending wait before pausing Tokio time. This excludes real startup and the shared cross-runtime CLI probe cache lock from virtual-time advancement while preserving the one-second close timeout and all custody/admission assertions. The CI log does not identify which acquire call timed out; the unsafe clock scope is established from the actual call graph, not a claimed local reproduction.

The existing five Windows acceptance setup/run steps may continue after a failed ordinary Test only for an explicit manual candidate dispatch with a successful workspace Build, no cancellation, and successful preceding setup stages. Test failure still fails the job. Normal PR validation and both approved SYSTEM conditions are unchanged. The archive.releaseName/Go-version compatibility changes remain included in this candidate. Native Windows execution, original launch-code diagnosis, SYSTEM acceptance and complete required PR checks remain unverified; macOS targeted diagnostic results are tracked separately.


### Fixed-source Windows launcher diagnosis after fc52cc2c

The fc52cc2c targeted Windows run 39038571381 passed ordinary Rust tests and reached the private-token launcher. Its retained artifact12103905889 proves SeImpersonatePrivilege was enabled, the private TokenOwner equaled TokenUser, and the original token stayed unchanged. CreateProcessWithTokenW returned Win32=5 (Access is denied), before an actual child handle was returned. The denied object is not established; this is not evidence for enabling privileges or changing ACLs. The earlier caaa macOS arm64 diagnostic package completed successfully without reproducing its DMG failure; no specific DMG root-cause fix is claimed.

The launcher now collects advisory read-only token default-DACL/session/integrity and object-security data, fixed executable/worker/configuration/working-directory security, current window-station/desktop security and Secondary Logon service state. Individual read failures remain evidence, cannot substitute for launch results and cannot alter existing permissions or the single child-creation attempt. Existing suspended-child checks, owner probe, job containment and cleanup remain intact.

An explicit workflow_dispatch boolean windows_launcher_diagnostics defaults false. Only that manual selection skips the ordinary expensive gate DAG and admits a dedicated Windows job, limited to polaris-arch/Polaris, collab/dots-audit-remediation-20261017 and reviewed kernel68ffb209ded1f9337f9115741cba24f9f4c63421. It invokes the same fixed launcher and target without a preceding Rust build/test, with read-only workflow permissions, nonpersistent checkout credentials and always-uploaded evidence. If creation succeeds the same target continues its native build/validation, which can be expensive and remains subject to its normal prerequisites. This diagnostic job never substitutes for required PR checks or SYSTEM acceptance; all existing PR/SYSTEM conditions remain unchanged. No new diagnostic result or access-denial fix is claimed before native execution.


### Bounded explicit-desktop experiment after 447cc50

The fast manual Windows run39041958153 again failed at CreateProcessWithTokenW with Win32=5 before a child was returned. Artifact12104840679 records enabled SeImpersonatePrivilege, unchanged original token, normalized private owner, matching session2/high integrity, and existing WinSta0/Default access entries. Secondary Logon was stopped with manual start; that observation does not establish causality. No denied object has been proven.

This candidate tests one documented API branch: explicitly select the already observed WinSta0\Default instead of a NULL desktop. Microsoft documents that NULL/empty desktop invokes automatic inherited-station/desktop permission handling. Before child creation, existing handles and names must read successfully and match WinSta0/Default exactly, and the current process plus original/private tokens must share the current process session (no prior-run numeric session is assumed); otherwise the experiment stops. The fixed string is not caller-supplied. No ACL, service, privilege, process API, target, suspended-child validation, job cleanup, PR condition or SYSTEM condition changes. This remains an unproven hypothesis, not an access-denial fix or acceptance PASS. The existing narrow manual job will provide the native result; a repeated Win32=5 rejects this experiment rather than authorizing additional permission changes.


### Offline AsUser candidate after 0ad5ee79

Round9 fast diagnostic run39043052501 retained the exact explicit-desktop source identity and still returned Win32=5 after all desktop/session preconditions. The desktop experiment did not resolve the launch denial. No denied object or service cause has been established.

This offline candidate replaces the single creation call with CreateProcessAsUserW using the same normalized private primary token, explicit executable/environment/working directory, verified desktop/session and fixed target. Process/thread security attributes are NULL and handle inheritance is false. The existing suspended-child owner checks, job containment, resume and cleanup remain. There is no fallback API, restricted-token substitution, new account right, ACL/service change or explicit privilege adjustment. The API may temporarily enable existing caller privileges for its call; SeIncreaseQuotaPrivilege was present but disabled, while SeAssignPrimaryTokenPrivilege was absent. A possible privilege-not-held result1314 must fail, without granting a missing right.

Complete caller privilege LUID/name identities and raw attribute masks are captured immediately before/after creation. LUID/name sets and all attribute bits except the known USED_FOR_ACCESS usage marker must match exactly. A successful suspended child is retained for cleanup before readback; changed state or failed readback stops execution before job assignment/resume, preserving both errors if creation also failed. Parent final evidence retains the privilege snapshots on success or failure. No manual restoration/mutation is attempted if Windows fails to restore state.

This is an offline source candidate. Executing the changed API requires explicit approval for the one-time GitHub runner and its temporary existing-privilege behavior. No native API run, full PR acceptance or SYSTEM PASS is claimed by local source-contract tests. Existing workflow selectors and PR/SYSTEM conditions are unchanged.


### Pure rebase onto published DNS fallback candidate

The five-file AsUser candidate is rebased onto App0e87449c8dacf23c6cf47bd8feba86e30d8fbf58/treec7f1f5b9070672f1b585d36a572aeb19e82f8436. All32 published DNS changes are retained byte-for-byte. The three reviewed launcher/script/test files retain the exact Round10 implementation bytes; only inventory/status integration metadata changes. Inventory now records the published DNS source identities as well. Rebase does not establish native AsUser, full PR or SYSTEM acceptance. Execution remains subject to the explicit approval required above.
