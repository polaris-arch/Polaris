# C4 next Mac observation: offline preparation

This directory contains a standard-library consistency checker, two proposed timing templates, a BPF record-accounting fixture, and one batch plan. It has no capture, SSH, privilege, device-run, Go-build, package, or installation entry point. It does not assess or change the user's existing authorization.

**Still unimplemented:** new Go observation hooks and their CGO1 build, signed candidate/package, next fixture/seal, revised runnable BPF observer with root-stage SHA/permissions, and a device runner for this plan. The old signed raw candidate cannot emit these proposed timing fields. `observation-plan.json` deliberately keeps these dependencies pending and `ready=false`.

Run only these local checks from the repository root:

```sh
PYTHONDONTWRITEBYTECODE=1 python3 -m unittest discover -s scripts/pc-mac-masque-next-observation-20260930/tests -v
PYTHONDONTWRITEBYTECODE=1 python3 scripts/pc-mac-masque-next-observation-20260930/verify.py capture scripts/pc-mac-masque-next-observation-20260930/fixtures/capture-records.json
PYTHONDONTWRITEBYTECODE=1 python3 scripts/pc-mac-masque-next-observation-20260930/verify.py trace scripts/pc-mac-masque-next-observation-20260930/fixtures/timing-send-template.json
PYTHONDONTWRITEBYTECODE=1 python3 scripts/pc-mac-masque-next-observation-20260930/verify.py trace scripts/pc-mac-masque-next-observation-20260930/fixtures/timing-recv-template.json
```

Exit 0 means a **synthetic logic fixture** is consistent. A well-formed `declared_unverified` document returns `review_required` and exit 2. Invalid or unsupported declarations return `unknown` and exit 2. Every result retains `logic_only=true`, `ready=false`, `device_pass=false`, `source_authentication=not_checked`, and `authorization_assessed=false`. Input hashes, source paths, process checks and pcap validity are reported facts supplied to this checker; it neither authenticates originals nor verifies kernel/runtime instrumentation. Hand-filling them cannot produce device PASS. It never emits NoOwner or network/FD/QUIC success.

## BPF record gate

`capture-records.json` is synthetic, including its process identity and source hashes. It deliberately uses `ps_recv=956550`, `captured=pcap_records=14` under a **different nonce** from the historical window. These numbers test arithmetic; they are not a replay receipt.

The gate requires the declared Mac142/en0/host-pair/port scope, exact tcpdump PID/uid/argv check, fresh descriptor and same final snapshot, known Darwin BPF counter meanings, no wrap/reset, ready then soft SIGINT and normal zero exit within 240/255 seconds, valid pcap, root/copy SHA agreement for ready/pcap/final, `0 < records < 256`, target Mac outbound UDP, `captured == records`, `ps_recv >= captured`, and zero reported BPF buffer drops. Missing counters, old `filter` names, zero packets, count cap, unsupported semantics, mismatched scopes, nonzero drops and abnormal exit remain unknown.

`capture_record_accounted=true` means only the supplied processed/written record counts agree. `ps_recv` counts before the filter; it is neither the target packet denominator nor a cross-observer delta. `kernel_drop=0` covers the BPF buffer counter, not NIC/driver losses. No result establishes complete wire capture, bidirectional 5-tuple coverage, FD delivery, readiness delivery, or QUIC success. Those gates and the existing formal classifier remain separate.

## Proposed timing fields and narrow templates

Each event carries one fixed `nonce`, PID, FD generation, FD, connection/transport/dial IDs, contiguous queue sequence, exact source-site label, and same-process monotonic `before_ns`/`after_ns` samples. Samples must bracket the observed operation, particularly the winning CAS or mutex assignment; an enqueue timestamp after the winner is insufficient. Nonoverlapping intervals establish the **required order**, not causal origin. Explicit `cause_seq`, `trigger_seq` and `winner_seq` are proposed source associations, not associations reconstructed from adjacent timestamps.

| Boundary | Required observation | Actual source to instrument later |
|---|---|---|
| RawRead | Whole original `rawConn.Read` enter/return, operation ID; EAGAIN callback false; callback reentry; return category and last syscall errno separately | `msgXReader.ReadBatch`, `quic-go/sys_conn_msgx_darwin.go` |
| Conn termination | Only successful `closeErr.CompareAndSwap(nil, e)` is the Conn winner; fixed reason and explicit observed error association | `Conn.setCloseError/destroyImpl`, `connection.go` |
| Transport termination | First `closeErr` assignment under its mutex is a separate winner | `Transport.close`, `transport.go` |
| Deadline | Paired ID, `createdConn`, set-now/restore-zero action, actual return result, explicit Transport association | `Transport.Close/maybeStopListening` |
| Dial exit | Exact selected branch and associated terminal winner, including all setup, transport-closed, context, err-channel, recreation, early-ready and handshake branches | `Transport.doDial` |

The plan requires all these sites, but this checker deliberately validates only two small templates:

- `send_epipe_after_wait`: RawRead enter → EAGAIN wait request → send EPIPE → Conn winner → explicitly associated Transport winner → successful set-now deadline → RawRead timeout return → associated doDial err-channel return. There is no callback retry in this template. This order does **not** prove the runtime entered `waitRead`, the deadline caused EPIPE, or any kernel/network cause.
- `recv_enotconn_before_send`: RawRead enter → recv ENOTCONN → RawRead returns nil native error with last syscall errno 57 → Transport winner → associated Conn winner → deadline → doDial err-channel return. This represents no observed send in this declared template; it cannot establish absence of packets outside it. `rawConn.Read` can return nil while `ReadBatch` subsequently wraps the callback errno, so those facts stay distinct.

Another branch, callback reentry, deadline restore, createdConn=true close path, overlapping intervals, extra events, or another actual order returns unknown for human review. Such output is not a failed product result. This is not an interpreter for the full old NDJSON stream; use the original classifier for that stream. Bounds are 512 events, 2 MiB input, 240 seconds total, and a proposed 3-second terminal-to-RawRead/doDial return observation limit. They do not change production deadlines or the existing frozen Stop budget. Missing/faulty finalization, dropped/late/write-error/identity-gap counters, hash mismatch and old coverage are unknown.

## One future batch, unchanged evidence boundaries

The plan references the existing preparation, runner, exact postcheck and formal classifier rather than copying them. After the pending instrumentation/package/observer review, bind a fresh nonce and all actual source/artifact hashes once, retain both capture-ready intervals, record a measured cross-host skew bound and a monotonic-to-wall-clock anchor, then run one fixed batch with the existing Stop/restore/exact-Clean sequence and independent two-host postcheck. Candidate interval coverage must include skew conservatively. Clock anchors or exact tuple association missing means unknown. Do not aggregate counters from different descriptors, interfaces, filters, windows or FD generations.

Historical `be243c8d` stays sealed: observer SHA `7ecc5fa3…` reports `unknown`; formal classifier exit 2 reports `usable=false`, `bidirectional outer UDP missing`. This checker rejects that nonce and does not edit its scripts, raw trace, pcap, receipts, or classification. HK01 success only excludes universal candidate failure. The LAN OpenClash QUIC-blocking hypothesis is outside this plan under the user's stated current configuration. No DNS, Go patch or nft candidate is consumed here.

Sources and exact existing paths are in `observation-plan.json`. The authoritative design is `/home/sway/docs/polaris/design/polaris-c4-mac-bpf-counter-gate-2026-09-30.md`; the evidence/source summary is `/home/sway/docs/polaris/design/polaris-hk01-c4-compare-2026-09-29.md`. The adjacent plan is a preparation artifact, not an updated global SoT or authorization receipt.
