# PC LAN echo source preparation

This namespace prepares a finite receiver/controller and checks bounded offline records. It does not run the application, core, helper, phone, network discovery or an installation. No device transition or real LAN socket has been exercised by its source tests. The existing authorized final batch remains the execution scope; preparation adds no authority and does not request another approval.

The accepted source field contract is `polaris-pc-echo-target-v1` / `polaris-pc-echo-public-witness-v1`. Root Android's exact field ACK is SHA256 `ea9b990b135f5854e57d8b31de95dd8e06e6d9aa2527b5f29b3f8ed81b1d69eb`, for proposal `067a2608695e4b1826ec51561d574c22ebeba66bdc032470bcc8c1bdcf13e2ed`. That ACK permits source preparation; final sender/receiver/package/current readiness and coordinated execution are not supplied by it. The scope SHA256 is `7d08501fbfb7d6269cf681d1de3f9d5e0138f3b89e4cd038de8de5ab372661c2`.

## Current execution boundary

**SOURCE_PREPARE_LIMITATION:** the public `serve` command fails with `ExecutionNotFrozen` before reading a plan, spawning a child or binding a socket. The existing root batch driver's exact external coordinated execution input/loader ABI is pending. A caller's `approved`, `actualInstalled`, source hash, version or JSON label cannot satisfy that seam. This is not a completed public run interface. Root must supply the independent reviewed source handoff, actual installed-package correspondence, current native admission and final window/matrix inputs; the loader must consume those existing receipts, rather than invent a second issuer or PKI. Android outbound remains denied until its own current admission and original dual-socket readiness are established.

The source contains actual IO mechanisms at `controller_run` and `receiver_loop`, for the future coordinated driver. They are not invoked by any offline CLI command. `controller_run` consumes the exact validated source snapshot, a bounded nonblocking `OutputQueue` over an actual pipe and a fixed snapshot/stop input; it launches only the fixed Python receiver runner. A blocking regular-file record sink is rejected before child creation. Only that original child may own the two receiver sockets. There is no API for arbitrary commands, downloaded code, hostnames, third targets, PID signaling, helper IPC or a core path probe.

Windows remains **source-prepared / runtime-unverified**. The Windows-only `WinFiles` backend in `lan.py` uses Python stdlib `ctypes` and existing kernel32/advapi32 functions, not `Add-Type`. It creates a new protected directory/file with an owner-only DACL at creation and validates the original HANDLE before writing secrets. It walks non-reparse path components and denies file write/delete sharing. Unsupported access/type/ACL/nonblocking-pipe behavior fails closed. `private-windows.ps1` is a fixed no-effect status shim, not an ACL certificate or a file creator; it is never launched by the controller. No Windows native DACL, HANDLE wait or exclusive-bind test has run. Darwin ACL and socket paths likewise have no native execution evidence.

## Offline commands

Use Python 3 with the standard library. All paths are operator inputs; they are not emitted in result records. Inputs must be bounded regular files in a private directory, with safe path components; POSIX private directories/files require actual owner-only mode and no extended ACL granting additional access. `prepare` creates a new private directory and file; it never overwrites or tightens an existing file. Create the private operator input through an existing trusted method. Do not place real native credentials in it.

For source observation/preparation, the two source items must also be placed by the coordinated owner under safe path components. A group-writable checkout such as the implementation workspace is deliberately rejected with `WritableParent`; invoking `source-hash` there is not a way to certify its source. Use the reviewed private source copy with the same two file names, verified against the external frozen bundle input. The tests use a new temporary fixture copy and do not chmod or change the workspace.

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -B REVIEWED_PRIVATE_COPY/lan.py source-hash
PYTHONDONTWRITEBYTECODE=1 python3 -B REVIEWED_PRIVATE_COPY/lan.py prepare \
  --input PRIVATE_OPERATOR_INPUT --output-directory NEW_PRIVATE_DIRECTORY
PYTHONDONTWRITEBYTECODE=1 python3 -B REVIEWED_PRIVATE_COPY/lan.py collect \
  --plan PRIVATE_PLAN --records PRIVATE_ORIGINAL_CONTROLLER_NDJSON \
  --witnesses PRIVATE_PUBLIC_WITNESS_ARRAY --cases PRIVATE_CASE_ARRAY --output NEW_PRIVATE_BUNDLE
PYTHONDONTWRITEBYTECODE=1 python3 -B REVIEWED_PRIVATE_COPY/lan.py verify \
  --plan PRIVATE_PLAN --bundle PRIVATE_BUNDLE
```

`prepare` emits `PreparedOnly`, no receiver/socket existence IDs. Both candidate SHAs are explicitly supplied final source candidates (40 lower hex characters), not Git HEAD defaults. The package hash must eventually correspond to root's independently obtained installed-package receipt; the plan cannot certify that correspondence. `collect` preserves each original canonical record byte string and its digest, and wraps no missing receipt or owner fact. Unknown remains incomplete. Return code 0 means only `LogicOnly` or `EvidenceConsistentUnverified`; it never means device acceptance. Return code 2 means `Invalid`, `Incomplete` or an unavailable execution seam.

The result union has exactly four assessment states: structural/binding/order contradiction → `Invalid`; missing original lifecycle/control/issuer evidence → `Incomplete`; complete synthetic input → `LogicOnly`; complete but offline origin-unverified input → `EvidenceConsistentUnverified`. All results have `claims=[]`, `globalNoOwner=false`, `managedReady=false`, `releaseReady=false`, `networkExact=false`, `outboundReady=false`. Ordinary native process wait and owned socket close do not prove platform resource retirement, global NoOwner, core path, DNS/WAN/NAT/DERP/TS, restore or release readiness.

## Bytes and typed records

All JSON schemas use exact keys/closed unions. Duplicate keys at any nesting level, unknown fields, bool as int, float/nonfinite numbers, invalid UTF-8 and size/depth limits are rejected. Canonical bytes mean UTF-8 output of sorted-key JSON, ASCII escaping, compact comma/colon separators and no final LF. `pcPlanSha256` is SHA256 of those exact canonical plan bytes. The NDJSON transport LF is not part of an individual raw record hash. `readyReceiptSha256` is SHA256 of the original receiver `Ready` record's canonical bytes, before controller base64 wrapping; it is not the controller wrapper or a reconstructed pretty JSON record.

The execution manifest has schema `polaris-pc-lan-execution-source-v1` and exactly two role/path/raw-file-SHA256 entries, in role order: `controller-receiver / lan.py`, `windows-private-file / private-windows.ps1`. `receiverSourceSha256` is SHA256 of ASCII `polaris-pc-lan-execution-source-v1`, one NUL byte, then the canonical manifest bytes without LF. No manifest/self hash is embedded in either source file. README, tests and fixture belong only to the five-file review inventory. Observing these selected bytes is self-consistency, not source or artifact authentication.

Private `PcEchoTarget` carries an independent 32..64 lowercase ASCII hex `echoNonce` and its hash. Public records carry only byte counts/digests and the closed root witness union `Unknown` or opaque `Reference{sha256}`. It never exposes native boot/session nonce, password, loan scope, native run/generation/birth/revision or probe port. PC run/plan, echo nonce and native admission remain distinct namespaces. The source does not translate this additive target into the old inbound-v2 protocol or a CAS credential.

Receiver records (`Ready`, `Counters`, `Request`, `Closed`, `Unknown`) can state only receiver-owned observations. Controller records (`ReceiverRecord`, `StopRequested`, `ChildWait`, `Unknown`) preserve original bounded pipe bytes plus contiguous sequences and record the separate original-child native wait. Receiver cannot mint `ChildWait`. Unknown is sticky across later observations. Snapshot records are not sealed; `Closed` counters are sealed. Same generation/socket identities and monotonically accumulated counters are mandatory.

The verifier binds every completed `Request` and every case's A/B/C/D snapshot to the original Ready's PC-domain lease: observed time must be at least its start and strictly less than expiry. At expiry and later is invalid business evidence. Missing `observedMonotonicNs` is a closed incomplete variant for Request/Counters only; the producer always supplies it, and verification never invents a timestamp. Closed, native wait, transport tail and snapshots not used as case windows may legitimately arrive/be observed later; they neither renew the lease nor prove timely business activity.

Each Request's counters must explain that one completed request relative to the preceding original counter-bearing receiver record: its protocol's total/category each +1 and, for matched, exactly echoWritten or writeFailed +1 according to echoState; everything else stays unchanged. Counters/Closed cannot introduce completed classifications or echoes. An accepted but unfinished request may only add equal total/pending debt and remains Incomplete, retaining Unknown and original lifecycle facts. Offline `collect` rejects malformed discriminators/base64 as a fixed typed Invalid result and does not write an output bundle for an Invalid assessment; it does not mask unrelated program defects.

## Receiver and custody rules

- Only AF_INET TCP and UDP, two distinct 49152..65535 ports, and explicit canonical RFC1918 unicast destination/expected sender. No discovery or name resolution. Ready follows actual TCP bind+listen, UDP bind, exact getsockname, both original object publication and installed deadline. Windows requires successful `SO_EXCLUSIVEADDRUSE` before bind; no reuse fallback.
- `FIRST_LF_FRAME_V1`: TCP interprets only the first LF and earlier bytes, at most 65 bytes. Segmentation and trailing bytes cannot change the first frame or cause a second echo. `NoLF`, early EOF, read error and absolute timeout are distinct. UDP has a 1500-byte capture bound; truncated/oversized consumption is not a full-datagram length/hash claim. Empty, foreign and malformed requests consume budget and never echo.
- One global budget of 32 actual TCP accepts / consumed UDP datagrams, counted before classification. No 33rd read. Counters conserve `total = pending + matched + wrongNonce + foreignPeer + readFailed + malformed`; `matched = echoWritten + writeFailed` after classification/write completes. Partial echo is still matched with failed write, not a full echo receipt. Foreign takes precedence over nonce mismatch.
- T0 precedes the first bind, with one immutable absolute lease of at most 120 seconds. Reads use `min(acceptedAt+1 second, lease end)`; fragmented input never extends it. A guardian closes original objects independently of record/stdout pressure. Records, controls, snapshots, queues and tail drain are bounded. There is no real-time guarantee when the OS suspends/deschedules execution; deadline alone never proves actual close.
- Unix uses one original `waitpid` path with true decoded status; ECHILD is permanently Unknown. Popen poll/wait/communicate/context/destructor cleanup and cached PID signaling are disabled. Windows observes only the original HANDLE completion. Ctrl-C, EOF and output/setup failures use bounded stop bytes or original control-pipe EOF. Unresolved child objects remain retained and block another controller admission; an irrecoverably killed custodian only leaves incomplete evidence. Owned socket close and child exit are separate facts. Nonzero native exit is preserved and is not normal completion.

## Proposed serial batch matrix

The source prepares six case IDs: `tcp-wrong-nonce`, `tcp-foreign-peer`, `tcp-wrong-port-admission` and their `udp-` counterparts. These IDs/matrix still need the coordinated root batch's exact source/window decision; the field ACK does not create execution readiness.

Each case requires original snapshot A → actual root Android before-positive → B → negative → C → actual root Android after-positive → D, all on the same original socket. Each positive must have total/matched/complete echo +1 and exact sent/returned byte hashes. Wrong nonce and foreign peer must actually reach that socket with total +1, the respective category +1, matched/echo +0. Missing foreign actor or an unobserved packet is incomplete; timeout is not a negative receipt. Wrong port is an Android exact-target **before-outbound admission rejection**, with `NotSent` / `NotReceived`; requested tuple uses the other already-approved protocol's port while retaining this protocol, and no third target is sent or bound. It is not a network-port denial test. Extra requests, unexplained deltas or generation changes invalidate a complete matrix.

PC snapshot order and Android attempt order are checked locally, with no cross-device clock comparison. A reused nonce is only correlation, not cryptographic request attribution. Receiver queues not actually accepted/read are unobserved. All sender/current native admission/installed-package evidence remains root's responsibility.

## Harmless source tests

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -B scripts/pc-lan-debug-20260930/tests/test_lan.py
```

This exact file is the closed test set. Socket, clock, pipe pressure and Windows security/handle cases are injected. The only actual child fixtures execute fixed Python `raise SystemExit(7)` with all streams disconnected; they test original native wait and externally reaped ECHILD and perform no networking. Synthetic records are explicitly manufactured and cannot enter device acceptance. The fixture includes a synthetic private plan nonce, not any native credential. No real listener, outbound connection, Android/Core/helper process, installation or host network change is run by the tests. No new package dependency or Cargo/Go build is needed.
