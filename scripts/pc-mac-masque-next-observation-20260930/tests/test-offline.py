import copy
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

HERE = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("next_observation", HERE / "verify.py")
gate = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gate)


def fixture(name):
    return json.loads((HERE / "fixtures" / name).read_text())


def seal(document):
    events = document["events"]
    document["telemetry"].update(events_sha256=gate.sha(events), emitted=len(events),
                                 enqueued=len(events), written=len(events))
    return document


class OfflineGates(unittest.TestCase):
    def assert_unknown(self, kind, document):
        result = gate.verify(kind, document)
        self.assertEqual(result["status"], "unknown", result)
        self.assertFalse(result["device_pass"])
        self.assertFalse(result["ready"])
        self.assertIsNone(result["network_success"])
        self.assertIsNone(result["fd_delivery_proven"])
        self.assertIsNone(result["quic_success"])
        self.assertFalse(result["formal_classifier_changed"])

    def test_prefilter_counter_can_exceed_processed_records_without_proving_delivery(self):
        document = fixture("capture-records.json")
        for ps_recv in (14, 956550):
            document["tcpdump_stats"]["ps_recv"] = ps_recv
            result = gate.verify("capture", document)
            self.assertTrue(result["capture_record_accounted"])
            self.assertEqual(result["status"], "synthetic_logic_consistent")
            self.assertTrue(result["logic_only"])
            self.assertFalse(result["ready"])
            self.assertFalse(result["device_pass"])
            self.assertIsNone(result["fd_delivery_proven"])

    def test_missing_negative_bool_string_and_old_counter_fields_fail_closed(self):
        base = fixture("capture-records.json")
        for key in ("captured", "ps_recv", "kernel_drop"):
            for value in (None, -1, True, "14", 14.0):
                with self.subTest(key=key, value=value):
                    document = copy.deepcopy(base)
                    document["tcpdump_stats"][key] = value
                    self.assert_unknown("capture", document)
            document = copy.deepcopy(base)
            del document["tcpdump_stats"][key]
            self.assert_unknown("capture", document)
        document = copy.deepcopy(base)
        document["tcpdump_stats"]["filter"] = document["tcpdump_stats"].pop("ps_recv")
        self.assert_unknown("capture", document)

    def test_counter_pcap_drop_cap_zero_and_outbound_negative_cases(self):
        for section, key, value in (
            ("tcpdump_stats", "captured", 13), ("tcpdump_stats", "ps_recv", 13),
            ("tcpdump_stats", "kernel_drop", 1), ("pcap", "records", 0),
            ("pcap", "records", 256), ("pcap", "mac_outbound_udp_records", 0),
            ("pcap", "mac_outbound_udp_records", 15), ("pcap", "structure_valid", False),
        ):
            with self.subTest(section=section, key=key, value=value):
                document = fixture("capture-records.json")
                document[section][key] = value
                self.assert_unknown("capture", document)

    def test_wrong_scope_identity_sources_wrap_reset_unknown_and_hash_mismatch(self):
        for section, key, value in (
            ("scope", "interface", "lo0"), ("scope", "device", "another-Mac"),
            ("scope", "port", 443), ("process", "pid", None), ("process", "uid", 501),
            ("process", "identity_checked", False), ("process", "argv", ["tcpdump"]),
            ("counter_source", "baseline", "unknown"), ("counter_source", "wrapped", True),
            ("counter_source", "reset", True), ("counter_source", "snapshot", "another-descriptor"),
            ("counter_source", "kernel_drop", "NIC_and_driver"),
        ):
            document = fixture("capture-records.json")
            document[section][key] = value
            self.assert_unknown("capture", document)
        document = fixture("capture-records.json")
        document["source_pairs"]["pcap"]["copy_sha256"] = "d" * 64
        self.assert_unknown("capture", document)
        del document["source_pairs"]["pcap"]
        self.assert_unknown("capture", document)
        document = fixture("capture-records.json")
        document["process"]["environment"] = {"secret": "do-not-echo"}
        self.assert_unknown("capture", document)
        self.assertNotIn("do-not-echo", json.dumps(gate.verify("capture", document)))

    def test_capture_readiness_exit_order_and_budget_negative_cases(self):
        for key, value in (("ready", False), ("soft_sigint_sent", False), ("normal_exit", False),
                           ("exit_code", 1), ("hard_timeout", True), ("ready_ns", 242_000_000_000),
                           ("soft_sigint_ns", 242_000_000_000), ("ended_ns", 256_000_000_000),
                           ("clock", "unknown")):
            document = fixture("capture-records.json")
            document["window"][key] = value
            self.assert_unknown("capture", document)

    def test_frozen_old_window_and_old_trace_cannot_be_reclassified(self):
        capture = fixture("capture-records.json")
        capture["scope"]["nonce"] = "be243c8d"
        self.assert_unknown("capture", capture)
        old = {"nonce": "be243c8d", "event": "recv_exit", "seq": 5, "errno": 35,
               "closeCoverage": "h3-transport-trackeddetach-socketowner-v1"}
        self.assert_unknown("trace", old)
        trace = fixture("timing-send-template.json")
        trace["source_manifest"]["coverage"] = old["closeCoverage"]
        self.assert_unknown("trace", trace)

    def test_two_fixed_timing_templates_are_logic_only(self):
        for name in ("timing-send-template.json", "timing-recv-template.json"):
            result = gate.verify("trace", fixture(name))
            self.assertEqual(result["status"], "synthetic_logic_consistent", result)
            self.assertFalse(result["causal_root_proven"])
            self.assertFalse(result["raw_read_kernel_wait_observed"])
            self.assertFalse(result["device_pass"])
            self.assertFalse(result["ready"])

    def test_wellformed_handfilled_real_declarations_require_review_and_never_device_pass(self):
        for kind, name in (("capture", "capture-records.json"), ("trace", "timing-send-template.json")):
            document = fixture(name)
            document["evidence_kind"] = "declared_unverified"
            if kind == "trace":
                document["source_manifest"]["instrumentation_status"] = "declared_unverified"
            result = gate.verify(kind, document)
            self.assertEqual(result["status"], "review_required", result)
            self.assertFalse(result["device_pass"])
            self.assertFalse(result["authorization_assessed"])
            self.assertEqual(result["source_authentication"], "not_checked")

    def test_missing_and_lost_telemetry_and_missing_source_fail_closed(self):
        for section, key, value in (("telemetry", "finalized", False), ("telemetry", "dropped", 1),
                                   ("telemetry", "written", 8), ("telemetry", "late_events", 1),
                                   ("telemetry", "events_sha256", "a" * 64),
                                   ("source_manifest", "prepare_sha256", None),
                                   ("source_manifest", "instrumentation_status", "implemented")):
            document = fixture("timing-send-template.json")
            document[section][key] = value
            self.assert_unknown("trace", document)

    def test_timing_critical_fields_and_explicit_links_are_required(self):
        for index, key, value in (
            (1, "callback_returns", True), (3, "won", False), (3, "cause_seq", 2),
            (4, "cause_seq", 3), (4, "domain", "conn"), (5, "trigger_seq", 4),
            (5, "created_conn", True), (6, "result", "error"), (7, "raw_read_id", 2),
            (7, "rawconn_error_kind", "nil"), (8, "branch", "handshake_complete"),
            (8, "winner_seq", 5), (2, "generation", 2), (2, "site", "unobserved writer"),
        ):
            with self.subTest(index=index, key=key):
                document = fixture("timing-send-template.json")
                document["events"][index][key] = value
                self.assert_unknown("trace", seal(document))

    def test_queue_reordering_reverse_overlap_and_independent_early_shutdown_are_unknown(self):
        document = fixture("timing-send-template.json")
        document["events"][0], document["events"][1] = document["events"][1], document["events"][0]
        self.assert_unknown("trace", seal(document))
        for index, before_ns, after_ns in ((4, 2_500_000, 2_501_000), (6, 8_500_000, 8_501_000),
                                          (3, 3_000_100, 4_001_000), (3, 5_000_000, 4_000_000)):
            document = fixture("timing-send-template.json")
            document["events"][index].update(before_ns=before_ns, after_ns=after_ns)
            self.assert_unknown("trace", seal(document))
        # Transport/deadline happened independently before EPIPE: timestamps cannot mint its cause chain.
        document = fixture("timing-send-template.json")
        document["events"][4].update(reason="recv-error", cause_seq=2)
        self.assert_unknown("trace", seal(document))

    def test_extra_callback_after_return_unsupported_branch_and_payload_fields_are_unknown(self):
        document = fixture("timing-send-template.json")
        retry = copy.deepcopy(document["events"][-1])
        retry.update(seq=10, event="raw_callback_retry", site=gate.SITES["raw_callback_retry"],
                     raw_read_id=1, callback_index=2)
        del retry["branch"], retry["winner_seq"]
        document["events"].append(retry)
        self.assert_unknown("trace", seal(document))
        document = fixture("timing-send-template.json")
        document["profile"] = "context_cancel"
        self.assert_unknown("trace", document)
        document = fixture("timing-send-template.json")
        document["events"][0]["payload"] = "do-not-echo-private-value"
        self.assert_unknown("trace", seal(document))
        self.assertNotIn("do-not-echo", json.dumps(gate.verify("trace", document)))

    def test_event_window_and_terminal_linkage_budget_are_strict(self):
        document = fixture("timing-send-template.json")
        document["events"] = document["events"] * 58
        self.assert_unknown("trace", seal(document))
        for offset in (4_000_000_000, 241_000_000_000):
            document = fixture("timing-send-template.json")
            document["events"][-1]["before_ns"] += offset
            document["events"][-1]["after_ns"] += offset
            self.assert_unknown("trace", seal(document))

    def test_cli_errors_do_not_echo_paths_values_or_unknown_evidence_kind(self):
        unknown = fixture("capture-records.json")
        unknown["evidence_kind"] = {"credential": "do-not-echo-private-value"}
        result = gate.verify("capture", unknown)
        self.assertEqual(result["evidence_kind"], "unknown")
        self.assertNotIn("private-value", json.dumps(result))
        proc = subprocess.run([sys.executable, str(HERE / "verify.py"), "trace",
                               "/missing/private-credential-path"], text=True, capture_output=True)
        self.assertEqual(proc.returncode, 2)
        self.assertNotIn("private-credential", proc.stdout + proc.stderr)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "input.json"
            for data in (b'{"private-key": 1, "private-key": 2}', b'{"n": NaN}',
                         b" " * (gate.MAX_BYTES + 1)):
                path.write_bytes(data)
                proc = subprocess.run([sys.executable, str(HERE / "verify.py"), "trace", str(path)],
                                      text=True, capture_output=True)
                self.assertEqual(proc.returncode, 2)
                self.assertNotIn("private-key", proc.stdout + proc.stderr)
            document = fixture("capture-records.json")
            document["evidence_kind"] = "declared_unverified"
            path.write_text(json.dumps(document))
            proc = subprocess.run([sys.executable, str(HERE / "verify.py"), "capture", str(path)],
                                  text=True, capture_output=True)
            self.assertEqual(proc.returncode, 2)
            self.assertEqual(json.loads(proc.stdout)["status"], "review_required")

    def test_plan_explicitly_leaves_instrumentation_package_and_device_runner_unimplemented(self):
        plan = json.loads((HERE / "observation-plan.json").read_text())
        self.assertFalse(plan["ready"])
        self.assertFalse(plan["new_instrumentation_implemented"])
        self.assertFalse(plan["new_package_exists"])
        self.assertFalse(plan["device_runner_implemented"])
        self.assertFalse(plan["authorization_assessed"])
        self.assertEqual(plan["window_count"], 1)
        self.assertIsNone(plan["next_candidate_sha256"])
        self.assertTrue(all(site["status"] == "not_implemented" for site in plan["instrumentation_sites"]))
        self.assertEqual(plan["historical_window"]["observer_status"], "unknown")
        self.assertFalse(plan["historical_window"]["formal_usable"])


if __name__ == "__main__":
    unittest.main()
