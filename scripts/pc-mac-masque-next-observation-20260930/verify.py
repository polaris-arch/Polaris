#!/usr/bin/env python3
"""Offline consistency gates. No process, device, network, or capture entry point."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import sys

MAX_BYTES = 2 * 1024 * 1024
MAX_EVENTS = 512
WINDOW_NS = 240_000_000_000
LINK_NS = 3_000_000_000
COVERAGE = "rawread-terminal-deadline-dodial-plan-v1"
OLD_NONCE = "be243c8d"
CAUSES = {"send-error", "recv-error", "context-cancel", "other"}
SITES = {
    "raw_read_enter": "msgXReader.ReadBatch/rawConn.Read",
    "raw_callback_retry": "msgXReader.ReadBatch/RawConn callback reentry",
    "recv_exit": "msgXReader.ReadBatch/recvmsg_x",
    "raw_wait_requested": "msgXReader.ReadBatch/EAGAIN callback false",
    "raw_read_return": "msgXReader.ReadBatch/rawConn.Read return",
    "send_exit": "oobConn.WritePacket/SendmsgN",
    "terminal_winner": None,  # The actual arbitration domain selects its source.
    "read_deadline_enter": "Transport.Close/SetReadDeadline",
    "read_deadline_return": "Transport.Close/SetReadDeadline",
    "do_dial_return": "Transport.doDial/selected return branch",
}


class Unknown(ValueError):
    pass


def need(condition, reason):
    if not condition:
        raise Unknown(reason)


def uint(value, name, minimum=0):
    need(type(value) is int and value >= minimum, f"missing/invalid {name}")
    return value


def sha(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True, separators=(",", ":"),
                                     allow_nan=False).encode()).hexdigest()


def digest(value):
    return isinstance(value, str) and re.fullmatch(r"[0-9a-f]{64}", value) is not None


def scope(document):
    need(document.get("evidence_kind") in {"synthetic", "declared_unverified"},
         "evidence kind must distinguish synthetic from unverified declarations")
    identity = document.get("scope")
    need(isinstance(identity, dict), "missing scope")
    need(isinstance(identity.get("nonce"), str) and
         re.fullmatch(r"[0-9a-f]{8}", identity["nonce"]), "invalid nonce")
    need(identity["nonce"] != OLD_NONCE, "frozen historical window cannot be reclassified")
    return identity


def capture_gate(document):
    need(set(document) == {"schema", "evidence_kind", "scope", "process", "counter_source",
                           "window", "tcpdump_stats", "pcap", "source_pairs"}, "capture fields missing/unknown")
    need(document.get("schema") == "c4-next-bpf-records-v1", "old/unknown capture schema")
    identity = scope(document)
    need(set(identity) == {"device", "nonce", "interface", "mac_ip", "peer_ip", "port"}, "capture scope fields differ")
    need(identity.get("device") == "Mac142" and identity.get("interface") == "en0" and
         identity.get("mac_ip") == "192.168.10.142" and
         identity.get("peer_ip") == "192.168.10.185", "wrong/unknown capture scope")
    port = uint(identity.get("port"), "port", 58000)
    need(port <= 60999, "port outside reviewed range")
    bpf = (f"(udp and ((src host 192.168.10.142 and dst host 192.168.10.185 and dst port {port}) "
           f"or (src host 192.168.10.185 and dst host 192.168.10.142 and src port {port}))) "
           "or (icmp and ((src host 192.168.10.142 and dst host 192.168.10.185) "
           "or (src host 192.168.10.185 and dst host 192.168.10.142)))")
    process = document.get("process", {})
    need(set(process) == {"pid", "uid", "euid", "identity_checked", "argv"}, "process fields missing/unknown")
    uint(process.get("pid"), "tcpdump PID", 1)
    output = f"/private/var/tmp/polaris-c4-mac-bpf-{identity['nonce']}/capture.pcap"
    expected = ["/usr/sbin/tcpdump", "-i", "en0", "-p", "-nn", "--immediate-mode",
                "-Z", "root", "-s", "96", "-c", "256", "-w", output, bpf]
    need(type(process.get("uid")) is int and process["uid"] == 0 and
         type(process.get("euid")) is int and process["euid"] == 0 and
         process.get("identity_checked") is True and process.get("argv") == expected,
         "exact tcpdump identity/argv not reported")
    source = document.get("counter_source", {})
    need(source == {"backend": "darwin_bpf_tcpdump", "ps_recv": "pre_filter_received",
                    "kernel_drop": "bpf_buffer_only", "baseline": "fresh_descriptor",
                    "snapshot": "same_descriptor_final", "wrapped": False, "reset": False},
         "counter semantics/baseline/wrap/reset unknown")
    need(source.get("wrapped") is False and source.get("reset") is False, "counter flags must be explicit booleans")
    window = document.get("window", {})
    need(set(window) == {"clock", "started_ns", "ready_ns", "soft_sigint_ns", "ended_ns",
                         "ready", "soft_sigint_sent", "hard_timeout", "normal_exit", "exit_code"},
         "window fields missing/unknown")
    times = [uint(window.get(key), key) for key in
             ("started_ns", "ready_ns", "soft_sigint_ns", "ended_ns")]
    need(window.get("clock") == "same_host_unix_ns" and
         times[0] <= times[1] < times[2] <= times[3], "capture timestamps missing/reversed")
    need(times[2] - times[0] <= WINDOW_NS and times[3] - times[0] < 255_000_000_000,
         "capture exceeds soft/hard budget")
    need(window.get("ready") is True and window.get("soft_sigint_sent") is True and
         window.get("hard_timeout") is False and window.get("normal_exit") is True and
         type(window.get("exit_code")) is int and window["exit_code"] == 0,
         "missing ready/soft stop/normal zero exit or hard timeout")
    stats = document.get("tcpdump_stats", {})
    need(set(stats) == {"captured", "ps_recv", "kernel_drop"}, "missing/old counter fields")
    captured, received, dropped = [uint(stats[key], key) for key in
                                   ("captured", "ps_recv", "kernel_drop")]
    pcap = document.get("pcap", {})
    need(set(pcap) == {"records", "mac_outbound_udp_records", "structure_valid"}, "pcap fields missing/unknown")
    records = uint(pcap.get("records"), "pcap_records", 1)
    outbound = uint(pcap.get("mac_outbound_udp_records"), "target outbound records", 1)
    need(pcap.get("structure_valid") is True and 0 < records < 256 and
         outbound <= records and captured == records and received >= captured and dropped == 0,
         "pcap/count/outbound/drop gate failed")
    hashes = document.get("source_pairs", {})
    need(set(hashes) == {"ready", "pcap", "final"}, "missing root/copy source pairs")
    for name, pair in hashes.items():
        need(isinstance(pair, dict) and set(pair) == {"root_path", "copy_path", "root_sha256", "copy_sha256"} and
             isinstance(pair.get("root_path"), str) and
             isinstance(pair.get("copy_path"), str) and pair["root_path"] != pair["copy_path"] and
             digest(pair.get("root_sha256")) and pair["root_sha256"] == pair.get("copy_sha256"),
             f"missing/mismatched {name} source pair")
    return {"capture_record_accounted": True,
            "ps_recv": received, "captured": captured, "pcap_records": records,
            "meaning": "reported processed/written records agree; no delivery completeness claim"}


def before(first, second):
    # A disjoint sampled interval establishes order. Queue sequence alone does not.
    return first["after_ns"] < second["before_ns"]


def trace_gate(document):
    """Two fixed proposed observation templates, not a causal inference engine."""
    need(document.get("schema") == "c4-next-timing-v1", "old/unknown trace schema")
    need(set(document) == {"schema", "evidence_kind", "profile", "scope", "clock",
                           "source_manifest", "telemetry", "events"}, "trace fields missing/unknown")
    identity = scope(document)
    need(set(identity) == {"nonce", "pid", "generation", "fd", "conn_id", "transport_id", "dial_id"},
         "trace scope fields differ")
    need(document.get("clock") == "same_process_monotonic_ns", "unknown trace clock")
    for key in ("pid", "generation", "fd"):
        uint(identity.get(key), key, 1)
    for key in ("conn_id", "transport_id", "dial_id"):
        need(isinstance(identity.get(key), str) and identity[key], f"missing {key}")
    provenance = document.get("source_manifest", {})
    need(set(provenance) == {"coverage", "prepare_sha256", "instrumentation_status"}, "source fields missing/unknown")
    need(provenance.get("coverage") == COVERAGE and digest(provenance.get("prepare_sha256")) and
         provenance.get("instrumentation_status") == document["evidence_kind"],
         "new instrumentation provenance missing (old syscall trace is insufficient)")
    events = document.get("events")
    need(isinstance(events, list) and 0 < len(events) <= MAX_EVENTS, "missing/over-budget events")
    final = document.get("telemetry", {})
    need(set(final) == {"finalized", "exit", "events_sha256", "emitted", "enqueued", "written",
                       "dropped", "write_errors", "identity_gaps", "late_events"}, "final fields missing/unknown")
    need(final.get("finalized") is True and final.get("exit") == "graceful" and
         final.get("events_sha256") == sha(events), "unfinalized/hash-mismatched trace")
    for key in ("emitted", "enqueued", "written"):
        need(type(final.get(key)) is int and final[key] == len(events), f"missing/mismatched {key}")
    for key in ("dropped", "write_errors", "identity_gaps", "late_events"):
        need(type(final.get(key)) is int and final[key] == 0, f"missing/nonzero {key}")
    common = {"seq", "event", "site", "before_ns", "after_ns", *identity}
    fields = {
        "raw_read_enter": {"raw_read_id"},
        "raw_wait_requested": {"raw_read_id", "errno", "callback_returns"},
        "raw_callback_retry": {"raw_read_id", "callback_index"},
        "raw_read_return": {"raw_read_id", "result", "rawconn_error_kind", "errno"},
        "send_exit": {"n", "errno", "io_id"},
        "recv_exit": {"n", "errno", "io_id", "raw_read_id"},
        "terminal_winner": {"domain", "reason", "won", "cause_seq"},
        "read_deadline_enter": {"deadline_id", "action", "created_conn", "trigger_seq"},
        "read_deadline_return": {"deadline_id", "result"},
        "do_dial_return": {"branch", "winner_seq"},
    }
    for index, event in enumerate(events, 1):
        need(isinstance(event, dict) and type(event.get("seq")) is int and
             event["seq"] == index, "trace sequence missing/reordered")
        name = event.get("event")
        need(name in fields and set(event) == common | fields[name], "unknown event/field or incomplete template")
        for key, value in identity.items():
            need(event.get(key) == value and type(event.get(key)) is type(value), f"event crosses {key} identity")
        uint(event["before_ns"], "event before_ns")
        uint(event["after_ns"], "event after_ns")
        need(event["before_ns"] <= event["after_ns"], "reversed event sample interval")
        expected_site = SITES.get(name)
        if name == "terminal_winner":
            expected_site = {"conn": "Conn.setCloseError/successful CAS",
                             "transport": "Transport.close/first assignment under mutex"}.get(event["domain"])
        need(expected_site is not None and event["site"] == expected_site, "event source/arbitration site differs")
    need(max(e["after_ns"] for e in events) - min(e["before_ns"] for e in events) <= WINDOW_NS,
         "trace exceeds observation time budget")

    def one(name, domain=None):
        matches = [e for e in events if e["event"] == name and
                   (domain is None or e.get("domain") == domain)]
        need(len(matches) == 1, f"missing/duplicate {name} {domain or ''}")
        return matches[0]

    def ordered(*rows):
        need(all(before(a, b) for a, b in zip(rows, rows[1:])),
             "required observation intervals overlap/reverse; queue order alone is insufficient")

    entered, returned = one("raw_read_enter"), one("raw_read_return")
    conn, transport = one("terminal_winner", "conn"), one("terminal_winner", "transport")
    deadline, deadline_return = one("read_deadline_enter"), one("read_deadline_return")
    dial = one("do_dial_return")
    uint(entered["raw_read_id"], "RawRead ID", 1)
    uint(returned["raw_read_id"], "RawRead return ID", 1)
    need(returned["raw_read_id"] == entered["raw_read_id"], "RawRead return is from another operation")
    need(returned["result"] in {"success", "timeout", "closed", "errno"} and
         returned["rawconn_error_kind"] in {"nil", "timeout", "closed", "other"}, "unknown RawRead return category")
    uint(returned["errno"], "RawRead last syscall errno")
    for winner in (conn, transport):
        need(winner["won"] is True and winner["reason"] in CAUSES, "attempt/ACK is not a recorded arbitration winner")
        ref = uint(winner["cause_seq"], "winner cause reference", 1)
        need(ref < winner["seq"], "winner reference is not an earlier observed event")
        ordered(events[ref - 1], winner)
    need(deadline["trigger_seq"] == transport["seq"] and deadline["created_conn"] is False and
         deadline["action"] == "set_now" and deadline["deadline_id"] == deadline_return["deadline_id"] and
         deadline_return["result"] == "ok", "deadline branch/result/explicit trigger linkage missing")
    uint(deadline["deadline_id"], "deadline ID", 1)
    uint(deadline_return["deadline_id"], "deadline return ID", 1)
    need(dial["branch"] == "err_chan" and dial["winner_seq"] == conn["seq"], "doDial selected branch/winner association differs")
    ordered(entered, returned)
    ordered(transport, deadline, deadline_return, dial)
    ordered(conn, dial)
    need(0 <= dial["after_ns"] - conn["before_ns"] <= LINK_NS and
         returned["after_ns"] - conn["before_ns"] <= LINK_NS, "RawRead/doDial exceeds terminal linkage budget")
    profile = document.get("profile")
    if profile == "send_epipe_after_wait":
        wait, error = one("raw_wait_requested"), one("send_exit")
        need(len(events) == 9 and wait["raw_read_id"] == entered["raw_read_id"] and
             wait["errno"] == 35 and wait["callback_returns"] is False and error["n"] == -1 and
             error["errno"] == 32, "EAGAIN/EPIPE template is incomplete or has unreviewed extra events")
        uint(error["io_id"], "send I/O ID", 1)
        need(conn["reason"] == "send-error" and conn["cause_seq"] == error["seq"] and
             transport["reason"] == "other" and transport["cause_seq"] == conn["seq"] and
             returned["result"] == "timeout" and returned["rawconn_error_kind"] == "timeout" and
             returned["errno"] == 35, "terminal/return categories do not match this template")
        ordered(entered, wait, error, conn, transport, deadline, deadline_return, returned, dial)
        ordering = "wait_request_then_EPIPE_winner_then_deadline_then_RawRead_return"
    elif profile == "recv_enotconn_before_send":
        error = one("recv_exit")
        need(len(events) == 8 and error["raw_read_id"] == entered["raw_read_id"] and
             error["n"] == -1 and error["errno"] == 57, "ENOTCONN template is incomplete or has extra events")
        uint(error["io_id"], "recv I/O ID", 1)
        need(transport["reason"] == "recv-error" and transport["cause_seq"] == error["seq"] and
             conn["reason"] == "recv-error" and conn["cause_seq"] == transport["seq"] and
             returned["result"] == "errno" and returned["rawconn_error_kind"] == "nil" and
             returned["errno"] == 57, "terminal/return categories do not match this template")
        ordered(entered, error, returned, transport, conn, deadline, deadline_return, dial)
        ordering = "ENOTCONN_then_transport_and_conn_winners_without_observed_send"
    else:
        raise Unknown("unsupported proposed template; requires human review")
    return {"template_order": ordering, "raw_read_kernel_wait_observed": False,
            "causal_root_proven": False,
            "meaning": "fixed explicitly linked user-space template only; no inferred kernel/network cause"}


def verify(kind, document):
    evidence = document.get("evidence_kind") if isinstance(document, dict) else None
    evidence = evidence if type(evidence) is str and evidence in {"synthetic", "declared_unverified"} else "unknown"
    result = {"device_pass": False, "network_success": None, "fd_delivery_proven": None,
              "quic_success": None, "formal_classifier_changed": False,
              "source_authentication": "not_checked", "authorization_assessed": False,
              "logic_only": True, "ready": False,
              "evidence_kind": evidence}
    try:
        need(kind in {"capture", "trace"}, "unknown offline checker kind")
        need(isinstance(document, dict), "input must be a JSON object")
        facts = capture_gate(document) if kind == "capture" else trace_gate(document)
        result.update(facts)
        result["status"] = "synthetic_logic_consistent" if document["evidence_kind"] == "synthetic" else "review_required"
    except (Unknown, KeyError, TypeError, AttributeError) as error:
        result.update(status="unknown", reason=str(error))
        if kind == "capture":
            result["capture_record_accounted"] = False
    return result


def no_duplicates(pairs):
    result = {}
    for key, value in pairs:
        need(key not in result, "duplicate JSON key")
        result[key] = value
    return result


def reject_constant(_):
    raise Unknown("non-finite JSON number")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("kind", choices=("capture", "trace"))
    parser.add_argument("input", type=Path)
    args = parser.parse_args()
    try:
        with args.input.open("rb") as stream:
            data = stream.read(MAX_BYTES + 1)
        need(len(data) <= MAX_BYTES, "input exceeds byte budget")
        document = json.loads(data, object_pairs_hook=no_duplicates,
                              parse_constant=reject_constant)
        result = verify(args.kind, document)
    except (OSError, ValueError, UnicodeError) as error:
        result = verify(args.kind, None)
        result["reason"] = f"input rejected: {type(error).__name__}"
    print(json.dumps(result, sort_keys=True, allow_nan=False))
    return 0 if result["status"] == "synthetic_logic_consistent" else 2


if __name__ == "__main__":
    sys.exit(main())
