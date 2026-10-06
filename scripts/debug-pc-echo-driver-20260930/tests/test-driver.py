"""LogicOnly closed importer/codec controls. No source runner, socket, process or device."""
import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("private_driver", Path(__file__).parents[1] / "driver.py")
D = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(D)


class PrivateImporterTests(unittest.TestCase):
    def test_strict_codec_rejects_duplicate_float_surrogate_overflow_and_boolean_sequence(self):
        for raw in (b'{"a":1,"a":2}', b'{"a":1.0}', b'{"a":NaN}', b'{"a":"\\ud800"}',
                    b'{"a":9223372036854775808}', b'{"a":"\xff"}'):
            with self.assertRaises(D.Unavailable):
                D.decode(raw)
        self.assertEqual({"a": 1}, D.decode(b'{"a":1}'))

    def test_private_mac_binds_original_canonical_body_and_key(self):
        key = bytes([9]) * 32
        body = {"requestId": "a" * 32, "sequence": 1}
        raw = D.envelope(key, body)
        self.assertEqual(body, D.authenticate(key, raw))
        with self.assertRaises(D.Unavailable):
            D.authenticate(bytes([8]) * 32, raw)
        value = D.decode(raw); value["mac"] = "0" * 64
        with self.assertRaises(D.Unavailable):
            D.authenticate(key, D.canonical(value))

    def test_current_lease_deducts_whole_pre_request_roundtrip_without_clock_comparison(self):
        self.assertEqual(799, D.remaining(1000, 5000000000, 5200000001))
        for args in ((1000, 500, 499), (1000, 0, 1000000000), (True, 0, 1), (120001, 0, 1)):
            with self.assertRaises(D.Unavailable):
                D.remaining(*args)

    def request(self):
        return {"schema": "polaris-debug-pc-echo-request-v1", "requestId": "a" * 32, "action": "ready", "phase": "ready",
                "sequence": 1, "bootNonce": "b" * 32, "sessionId": "c" * 32, "nonce": "d" * 48,
                "planSha256": "e" * 64, "apkSha256": "f" * 64, "expectedSourcePin": "1" * 64}

    def test_fixed_matrix_and_external_source_package_binding_reject_self_approval(self):
        first = self.request()
        self.assertEqual(24, len(D.PHASES))
        self.assertEqual(first, D.request_check(first, 1, None, "1" * 64, "f" * 64))
        for key, value in (("approved", True), ("sequence", True), ("apkSha256", "0" * 64),
                           ("expectedSourcePin", "0" * 64), ("phase", "tcp-any-host")):
            with self.assertRaises(D.Unavailable):
                D.request_check({**first, key: value}, 1, None, "1" * 64, "f" * 64)
        second = {**first, "sequence": 2, "action": "snapshot", "phase": D.PHASES[0], "requestId": "2" * 32}
        self.assertEqual(second, D.request_check(second, 2, first, "1" * 64, "f" * 64))
        with self.assertRaises(D.Unavailable):
            D.request_check({**second, "nonce": "0" * 48}, 2, first, "1" * 64, "f" * 64)

    def test_source_prepare_run_gate_precedes_source_loader_package_child_or_device(self):
        with patch.dict(D.os.environ, {"POLARIS_NO_KERNEL_RUN": "1"}), patch.object(D, "load_original_sources") as loader:
            with self.assertRaises(D.Unavailable):
                D.run(None)
            loader.assert_not_called()

    def test_actual_installed_bytes_require_the_independent_build_artifact_receipt(self):
        raw = b'LogicOnly external artifact fixture'
        binding = {"androidCandidateSha": "1" * 40, "androidPackageSha256": D.sha(raw)}
        installed = {"package": D.PACKAGE, "installedBytes": raw, "externalArtifactBytes": raw, "sha256": D.sha(raw)}
        receipt = {"commit": "1" * 40, "packageArtifact": {"sha256": D.sha(raw), "bytes": len(raw),
                    "packageName": D.PACKAGE, "variant": "Debug"}}
        D.package_binding(receipt, binding, installed)
        for bad in ({"commit": "1" * 40, "actualInstalled": True}, {**receipt, "commit": "0" * 40},
                    {**receipt, "packageArtifact": {**receipt["packageArtifact"], "bytes": True}},
                    {**receipt, "packageArtifact": {**receipt["packageArtifact"], "sha256": "0" * 64}}):
            with self.assertRaises(D.Unavailable):
                D.package_binding(bad, binding, installed)


from contextlib import ExitStack
import base64
import copy
import threading

LAN_SPEC = importlib.util.spec_from_file_location("driver_original_lan", Path(__file__).parents[2] / "pc-lan-debug-20260930" / "lan.py")
L = importlib.util.module_from_spec(LAN_SPEC)
LAN_SPEC.loader.exec_module(L)


class InjectedDriver:
    """LogicOnly: real ordered worker/receiver codecs, entirely fake effects and clock.

    OperatorDeclared is the required wire discriminator being tested, not a claim
    about this fixture's origin or runtime admission. No child/controller is run.
    """
    def __init__(self, mode=True, fault=None, mutate=None, budget=32):
        self.mode, self.fault, self.mutate, self.budget = mode, fault, mutate, budget
        self.now = 1000000000
        self.secret = bytes([9]) * 32
        self.stop = threading.Event()
        self.foreign, self.output, self.replies, self.sockets, self.controls = {}, [], [], [], []
        self.position, self.cancelled, self.emit_calls = 0, False, 0
        self.sleeps, self.duplicate_seen = [], False
        self.ids = 0
        binding = {"runId": "a" * 32, "pcCandidateSha": "1" * 40, "androidCandidateSha": "2" * 40,
            "receiverSourceSha256": "3" * 64, "androidPackageSha256": "f" * 64,
            "destinationIPv4": "192.168.8.10", "expectedSenderIPv4": "192.168.8.11",
            "tcpPort": 50001, "udpPort": 51003, "nonceSha256": D.sha(b"b" * 32),
            "lifetimeSeconds": 120, "maxPayloadBytes": 65, "maxConnectionsOrDatagrams": 32}
        if fault == "same_source":
            binding["expectedSenderIPv4"] = binding["destinationIPv4"]
        self.plan = {"schema": "polaris-pc-lan-plan-v1", "scopeSha256": L.SCOPE_SHA, "binding": binding,
            "ownerInput": {"kind": "OperatorDeclared", "destinationIPv4": binding["destinationIPv4"]},
            "echoNonce": "b" * 32, "sourceKind": "OperatorDeclared"}
        raw_plan = D.canonical(self.plan)
        self.records = D.OriginalRecords(L, raw_plan, {}, [])
        self.records.pc_source_sha, self.records.driver_source_sha = "4" * 64, "5" * 64
        self.receiver = L.Receiver(self.plan, L.plan_sha(raw_plan), "6" * 32, self.receive,
                                   factory=self.receiver_socket, clock=lambda: self.now / 1e9, windows=False)
        first = PrivateImporterTests().request()
        self.requests = [D.envelope(self.secret, {**first, "sequence": seq, "requestId": format(seq, "032x"),
            "action": "ready" if seq == 1 else "snapshot", "phase": "ready" if seq == 1 else D.PHASES[seq - 2]})
            for seq in range(1, 26)]

    def token(self, size):
        self.ids += 1
        return format(self.ids, "0%dx" % (size * 2))

    def unlocked(self):
        assert not self.records.condition._is_owned()
        assert all(not entry["custody"].lock.locked() for entry in self.foreign.values() if "custody" in entry)

    def receive(self, raw):
        if self.mutate:
            raw = D.canonical(self.mutate(D.decode(raw), self.position + 1))
        value = {"schema": "polaris-pc-lan-controller-v1", "role": "Controller", "phase": "ReceiverRecord",
            "controllerInstanceId": "6" * 32, "eventSeq": self.records.event_sequence + 1,
            "sourceKind": "OperatorDeclared", "receiverRaw": base64.b64encode(raw).decode("ascii"),
            "receiverRawSha256": D.sha(raw)}
        self.records.receive(D.canonical(value))

    def receiver_socket(self, *args):
        class ReceiverSocket:
            def setblocking(self, value): pass
            def bind(self, address): self.address = address
            def listen(self, count): pass
            def getsockname(self): return self.address
            def close(self): pass
        return ReceiverSocket()

    def actor_socket(self, family, kind):
        self.unlocked()
        assert family == D.socket.AF_INET and kind in (D.socket.SOCK_STREAM, D.socket.SOCK_DGRAM)
        handle = InjectedSocket(self, "tcp" if kind == D.socket.SOCK_STREAM else "udp")
        self.sockets.append(handle)
        if self.fault == "cancel_constructor": self.cancelled = True
        return handle

    def request(self):
        self.unlocked()
        sequence = self.position + 1
        point = ("constructor" if sequence == 7 and "tcp" in self.foreign and not self.sockets else
                 "send" if sequence == 7 and self.sockets and hasattr(self.sockets[0], "target") and not self.sockets[0].sends else
                 "c" if sequence == 8 and self.receiver.snapshots == 7 else None)
        if self.fault == "stop_read_" + str(point): self.stop.set()
        if self.fault == "identity_read_constructor" and point == "constructor": self.records.controller_id = "a" * 32
        if self.fault == "c_read_clock" and point == "c": self.now += 2000000000
        if self.fault == "carry_b_deadline" and sequence == 8 and self.receiver.snapshots == 6:
            self.now += 1500000000
        if self.fault == "duplicate_negative" and self.position == 7 and not self.duplicate_seen:
            self.duplicate_seen = True
            return self.requests[6]
        return None if self.cancelled else self.requests[self.position]

    def reply(self, raw):
        self.unlocked()
        if self.position + 1 == 8:
            if self.fault == "reply_clock": self.now += 2000000000
            if self.fault == "reply_stop": self.stop.set()
        self.replies.append(D.authenticate(self.secret, raw))
        self.position += 1

    def write(self, fd, raw):
        self.unlocked()
        self.controls.append(raw)
        value = D.decode(raw.rstrip(b"\n"))
        if value["command"] == "readyHandoff":
            # Simulate prior requests only after original zero-counter Ready.
            self.receiver.requests = 32 - self.budget
            self.receiver.counters["tcp"]["total"] = 32 - self.budget
            self.receiver.counters["tcp"]["malformed"] = 32 - self.budget
            self.receiver.ready_handoff(value["requestId"])
        elif value["command"] == "snapshot":
            if self.position + 1 == 7 and self.fault == "delayed_b": self.now += 750000000
            self.receiver.snapshot()
            if self.position + 1 == 7:
                if self.fault == "cancel_snapshot": self.cancelled = True
                if self.fault == "stop_snapshot": self.stop.set()
                if self.fault == "closed_snapshot": self.records.closed = True
                if self.fault == "expired_snapshot": self.now += 2000000000
            if self.position + 1 == 8 and self.fault == "cancel_c": self.cancelled = True
            if self.position + 1 == 8:
                if self.fault == "c_delivery_clock": self.now += 2000000000
                if self.fault == "carry_b_deadline": self.now += 750000000
        else:
            assert value == {"command": "stop"}
        return len(raw)

    def emit(self, record):
        self.unlocked(); self.emit_calls += 1
        if self.fault == "output_failure": raise OSError("injected sidecar write failure")
        self.output.append(copy.deepcopy(record))

    def wait_tick(self, timeout):
        self.now += 2000000000  # Bounded fake wait, never a real blocking wait.

    def run(self):
        fixture = self
        class Guardian:
            def __init__(self, **kwargs): self.name = kwargs["name"]; self.started = False
            def start(self): fixture.unlocked(); self.started = True
            def join(self, timeout):
                fixture.unlocked(); assert timeout == 1
                if fixture.fault == "join_clock" and self.name == "pc-local-foreign-deadline": fixture.now += 2000000000
            def is_alive(self): return fixture.fault == "join_unknown" and self.name == "pc-local-foreign-deadline"
        with ExitStack() as stack:
            stack.enter_context(patch.object(D.time, "monotonic_ns", side_effect=lambda: self.now))
            stack.enter_context(patch.object(D.time, "sleep", side_effect=self.sleeps.append))
            stack.enter_context(patch.object(D.threading, "Thread", Guardian))
            stack.enter_context(patch.object(L.secrets, "token_hex", side_effect=self.token))
            stack.enter_context(patch.object(D.socket, "socket", side_effect=self.actor_socket))
            stack.enter_context(patch.object(D.os, "write", side_effect=self.write))
            stack.enter_context(patch.object(D.subprocess, "Popen", side_effect=AssertionError("process forbidden")))
            stack.enter_context(patch.object(self.records.condition, "wait", side_effect=self.wait_tick))
            self.receiver.setup()
            self.now += 1000000
            D.serve_private_requests(self.records, self, self.secret, 99, self.stop, "1" * 64, "f" * 64,
                                     self.foreign if self.mode else None, self.emit if self.mode else None)


class InjectedSocket:
    def __init__(self, fixture, protocol):
        self.f, self.protocol, self.data, self.sends, self.closes = fixture, protocol, bytearray(), 0, 0
    def settimeout(self, value): self.f.unlocked(); assert 0 < value <= 1
    def bind(self, address):
        self.f.unlocked(); assert address == ("192.168.8.10", 0)
        self.local = (address[0], 54000 + len(self.f.sockets))
    def getsockname(self):
        self.f.unlocked()
        if self.f.fault == "bad_source": return ("192.168.8.12", self.local[1])
        if self.f.fault == "bad_port": return (self.local[0], 0)
        return self.local
    def connect(self, target): self.f.unlocked(); self.target = target
    def getpeername(self):
        self.f.unlocked()
        return (self.target[0], self.target[1] + 1) if self.f.fault == "bad_target" else self.target
    def send(self, payload):
        self.f.unlocked(); self.sends += 1
        if self.f.fault == "send_error": raise OSError("injected write with no returned count")
        if self.f.fault == "partial_tcp" and self.sends == 2: raise OSError("injected partial write")
        count = 5 if self.f.fault in ("partial_tcp", "partial_udp") else len(payload)
        self.data.extend(payload[:count])
        if self.f.fault == "cancel_send": self.f.cancelled = True
        complete = self.protocol == "udp" or self.data.endswith(b"\n")
        if complete and self.f.fault != "missing_request":
            self.f.receiver.begin_request(self.protocol)
            self.f.receiver.request(self.protocol, bytes(self.data), "Complete", self.local[0],
                lambda data: (_ for _ in ()).throw(AssertionError("foreign must never echo")),
                lambda timeout: False, self.f.now / 1e9 + 1)
        return count
    def recv(self, count):
        self.f.unlocked(); assert count == 1501
        if self.f.fault == "read_error": raise OSError("injected read failure")
        if self.f.fault == "returned_bytes": return b"unexpected"
        if self.f.fault == "expired_read": self.f.now += 2000000000
        if self.protocol == "udp": raise D.socket.timeout()
        return b""
    def close(self):
        self.f.unlocked(); self.closes += 1
        if self.f.fault == "close_clock": self.f.now += 2000000000
        if self.f.fault == "close_unknown": raise OSError("injected close uncertainty")


class PcLocalForeignTests(unittest.TestCase):
    def reject(self, fixture):
        with self.assertRaises((D.Unavailable, L.Fault, OSError, KeyError)):
            fixture.run()
        self.assertTrue(all(item["status"] == "Unknown" for item in fixture.output))
        self.assertTrue(all(item["record"]["status"] == "Unknown" for item in fixture.foreign.values()))
        return fixture

    def test_original_worker_tcp_udp_fixed_phases_exact_raw_progress_and_counter_delta(self):
        fixture = InjectedDriver(); fixture.run()
        self.assertEqual(25, len(fixture.replies)); self.assertEqual(24, fixture.receiver.snapshots)
        self.assertEqual(2, fixture.receiver.requests)
        self.assertEqual([7, 19], [record["phaseSequence"] for record in fixture.output])
        self.assertEqual(["tcp", "udp"], [record["protocol"] for record in fixture.output])
        self.assertEqual(2, len(fixture.sockets))
        for record, socket in zip(fixture.output, fixture.sockets):
            self.assertEqual("ObservedPcLocalSourceRejection", record["status"])
            self.assertEqual({"address": "192.168.8.10", "port": socket.local[1]}, record["actualSourceTuple"])
            self.assertEqual(1, socket.sends); self.assertEqual(1, socket.closes)
            self.assertTrue(record["guardianJoined"]); self.assertEqual("Closed", record["socketClose"])
            payload = b"b" * 32 + (b"\n" if record["protocol"] == "tcp" else b"")
            self.assertEqual({"kind": "Bytes", "count": len(payload), "sha256": D.sha(payload)}, record["sent"])
            self.assertEqual({"kind": "NotReceived"}, record["returned"])
            for name in ("B", "Request", "C"):
                raw = L.raw_base64(record["original" + name])
                self.assertEqual(D.sha(raw), record["original" + name + "Sha256"])
                self.assertTrue(any(raw == original for original, _ in fixture.records.receiver))
            before, request, after = [L.receiver_record(L.raw_base64(record["original" + name])) for name in ("B", "Request", "C")]
            self.assertEqual(before["receiverSeq"] + 1, request["receiverSeq"])
            self.assertEqual(request["receiverSeq"] + 1, after["receiverSeq"])
            self.assertEqual(request["counters"], after["counters"])
            D.foreign_counter_delta(before["counters"], after["counters"], record["protocol"])
            self.assertEqual("foreignPeer", request["request"]["classification"])
            self.assertEqual("NotAttempted", request["request"]["echoState"])
        self.assertEqual(b'{"command":"stop"}\n', fixture.controls[-1])

    def test_default_same_original_twentyfive_reply_loop_has_no_actor(self):
        fixture = InjectedDriver(mode=False); fixture.run()
        self.assertEqual(25, len(fixture.replies)); self.assertEqual(24, fixture.receiver.snapshots)
        self.assertEqual([], fixture.sockets); self.assertEqual([], fixture.output)
        self.assertEqual(0, fixture.receiver.requests)

    def test_import_has_no_socket_process_pipe_or_thread_effects(self):
        with patch.object(D.socket, "socket") as socket, patch.object(D.subprocess, "Popen") as process, \
                patch.object(D.os, "pipe") as pipe, patch.object(D.threading, "Thread") as thread:
            spec = importlib.util.spec_from_file_location("pure_driver_import", SPEC.origin)
            module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
            for effect in (socket, process, pipe, thread): effect.assert_not_called()

    def test_explicit_mode_cannot_bypass_before_effects_run_gate(self):
        with patch.dict(D.os.environ, {"POLARIS_NO_KERNEL_RUN": "1"}), patch.object(D, "load_original_sources") as loader, \
                patch.object(D, "AdbPrivateChannel") as adb, patch.object(D.socket, "socket") as socket, \
                patch.object(D.threading, "Thread") as thread:
            with self.assertRaises(D.Unavailable): D.run(type("Args", (), {"pc_local_foreign": True})())
            for effect in (loader, adb, socket, thread): effect.assert_not_called()

    def test_handoff_budget_fifteen_denies_before_actor_sixteen_uses_original_global_budget(self):
        low = self.reject(InjectedDriver(budget=15))
        self.assertEqual([], low.sockets); self.assertEqual([], low.replies)
        enough = InjectedDriver(budget=16); enough.run()
        self.assertEqual(16, enough.replies[0]["remainingRequests"])
        self.assertEqual(18, enough.receiver.requests); self.assertEqual(24, enough.receiver.snapshots)
        self.assertEqual(32, enough.plan["binding"]["maxConnectionsOrDatagrams"])

    def test_actual_source_and_approved_peer_drift_deny_before_send(self):
        for fault in ("same_source", "bad_source", "bad_port", "bad_target"):
            with self.subTest(fault=fault):
                fixture = self.reject(InjectedDriver(fault=fault))
                self.assertTrue(all(handle.sends == 0 for handle in fixture.sockets))
                self.assertTrue(all(handle.closes == 1 for handle in fixture.sockets))

    def test_current_authenticated_source_package_phase_and_session_drift_deny_before_actor(self):
        for key, value in (("expectedSourcePin", "0" * 64), ("apkSha256", "0" * 64),
                           ("phase", "udp-foreign-peer-negative"), ("sessionId", "0" * 32)):
            with self.subTest(key=key):
                fixture = InjectedDriver()
                body = D.authenticate(fixture.secret, fixture.requests[6]); body[key] = value
                fixture.requests[6] = D.envelope(fixture.secret, body)
                self.reject(fixture)
                self.assertEqual([], fixture.sockets); self.assertEqual(6, len(fixture.replies))

    def test_duplicate_original_negative_request_does_not_repeat_send_or_snapshot(self):
        fixture = InjectedDriver(fault="duplicate_negative"); fixture.run()
        self.assertTrue(fixture.duplicate_seen); self.assertEqual([.02], fixture.sleeps)
        self.assertEqual(24, fixture.receiver.snapshots); self.assertEqual(2, fixture.receiver.requests)
        self.assertEqual([1, 1], [socket.sends for socket in fixture.sockets])

    def test_pre_b_deadline_covers_snapshot_time_and_does_not_start_new_two_seconds(self):
        fixture = InjectedDriver(fault="delayed_b"); fixture.run()
        record = fixture.output[0]; before = L.receiver_record(L.raw_base64(record["originalB"]))
        deadline = fixture.foreign["tcp"]["custody"].deadline
        self.assertEqual(3.001, deadline)
        self.assertLess(deadline, before["observedMonotonicNs"] / 1e9 + 2)
        self.assertEqual(121000000000, L.receiver_record(fixture.records.ready_raw)["expiresMonotonicNs"])

    def test_cancellation_stop_closed_and_expiry_do_not_reply_or_reacquire(self):
        for fault in ("cancel_snapshot", "cancel_constructor", "cancel_send", "stop_snapshot", "closed_snapshot",
                      "expired_snapshot", "expired_read", "cancel_c"):
            with self.subTest(fault=fault):
                fixture = self.reject(InjectedDriver(fault=fault))
                self.assertEqual(7 if fault == "cancel_c" else 6, len(fixture.replies))
                self.assertLessEqual(len(fixture.sockets), 1)
                self.assertTrue(all(handle.closes == 1 for handle in fixture.sockets))
                if fault == "cancel_c": self.assertIn("originalC", fixture.output[0])

    def test_partial_tcp_write_preserves_actual_prefix_without_retry(self):
        fixture = self.reject(InjectedDriver(fault="partial_tcp"))
        record = fixture.output[0]
        self.assertEqual({"kind": "Unknown", "confirmedPrefix": {"kind": "Bytes", "count": 5,
                         "sha256": D.sha(b"b" * 5)}}, record["sent"])
        self.assertTrue(record["sendAttempted"]); self.assertFalse(record["sendComplete"])
        self.assertEqual(2, fixture.sockets[0].sends); self.assertEqual("Closed", record["socketClose"])
        self.assertEqual(0, fixture.receiver.requests)

    def test_partial_udp_datagram_is_never_resent(self):
        fixture = InjectedDriver(fault="partial_udp")
        # Allow TCP to finish; only the UDP physical write is partial.
        original = InjectedSocket.send
        def send(socket, payload):
            fault = socket.f.fault
            if socket.protocol == "tcp": socket.f.fault = None
            try: return original(socket, payload)
            finally: socket.f.fault = fault
        with patch.object(InjectedSocket, "send", send):
            with self.assertRaises(D.Unavailable): fixture.run()
        self.assertEqual(1, fixture.sockets[1].sends)
        self.assertEqual(5, fixture.foreign["udp"]["record"]["sent"]["count"])
        self.assertFalse(fixture.foreign["udp"]["record"]["sendComplete"])
        self.assertEqual("Unknown", fixture.foreign["udp"]["record"]["status"])

    def test_no_request_no_echo_read_error_or_unjoined_custody_never_prove_rejection(self):
        for fault in ("send_error", "missing_request", "read_error", "returned_bytes", "close_unknown", "join_unknown"):
            with self.subTest(fault=fault):
                fixture = self.reject(InjectedDriver(fault=fault))
                self.assertEqual(6, len(fixture.replies)); self.assertEqual(1, len(fixture.sockets))
                if fault == "close_unknown":
                    entry = fixture.foreign["tcp"]
                    self.assertEqual("Unknown", entry["record"]["socketClose"])
                    self.assertIs(fixture.sockets[0], entry["custody"].handles[0][1])
                if fault == "join_unknown": self.assertFalse(fixture.output[0]["guardianJoined"])
                if fault == "send_error":
                    self.assertEqual({"kind": "Unknown", "confirmedPrefix": {"kind": "NotSent"}}, fixture.output[0]["sent"])
                    self.assertTrue(fixture.output[0]["sendAttempted"])

    def test_request_identity_frame_or_time_mutations_and_extra_c_counter_deny(self):
        def mutate(kind):
            def change(item, seq):
                if item["phase"] == "Request":
                    if kind == "socket": item["request"]["socketInstanceId"] = "a" * 32
                    if kind == "peer": item["request"]["peerIPv4"] = "192.168.8.12"
                    if kind == "frame": item["request"]["frame"]["sha256"] = "0" * 64
                    if kind == "time": item.pop("observedMonotonicNs")
                    if kind == "receiver": item["receiverInstanceId"] = "a" * 32
                    if kind == "replay": item["receiverSeq"] -= 1
                if item["phase"] == "Counters" and seq == 8 and kind == "extra_c":
                    item["counters"]["udp"]["total"] += 1; item["counters"]["udp"]["malformed"] += 1
                return item
            return change
        for kind in ("socket", "peer", "frame", "time", "receiver", "replay", "extra_c"):
            with self.subTest(kind=kind): self.reject(InjectedDriver(mutate=mutate(kind)))

    def test_original_membership_handoff_and_unsealed_b_are_required_before_socket(self):
        fixture = InjectedDriver(); fixture.run()
        before_raw = L.raw_base64(fixture.output[0]["originalB"])
        for mutate in (lambda: setattr(fixture.records, "handoff_raw", None),
                       lambda: setattr(fixture.records, "closed", True),
                       lambda: fixture.records.receiver.clear()):
            original = (fixture.records.handoff_raw, fixture.records.closed, list(fixture.records.receiver))
            mutate()
            with self.assertRaises(D.Unavailable): D.foreign_business_record(fixture.records, before_raw, "Counters")
            fixture.records.handoff_raw, fixture.records.closed, fixture.records.receiver = original

    def test_missing_b_time_sealed_b_or_unknown_record_fields_do_not_acquire_actor(self):
        for fault in ("time", "sealed", "extra"):
            def mutate(item, seq):
                if item["phase"] == "Counters" and seq == 7:
                    if fault == "time": item.pop("observedMonotonicNs")
                    if fault == "sealed": item["sealed"] = True
                    if fault == "extra": item["approved"] = True
                return item
            with self.subTest(fault=fault):
                fixture = self.reject(InjectedDriver(mutate=mutate))
                self.assertEqual([], fixture.sockets)

    def test_failed_sidecar_write_is_unknown_and_not_retried(self):
        fixture = self.reject(InjectedDriver(fault="output_failure"))
        self.assertEqual(1, fixture.emit_calls); self.assertEqual([], fixture.output)
        self.assertEqual("Unknown", fixture.foreign["tcp"]["record"]["status"])
        self.assertEqual(1, len(fixture.sockets))

    def test_sidecar_finite_local_scope_omits_nonce_key_and_native_or_device_grants(self):
        fixture = InjectedDriver(); fixture.run()
        self.assertEqual(2, len(fixture.output))
        for record in fixture.output:
            self.assertEqual("PcLocalIPv4ForeignSourceOnly", record["scope"])
            self.assertEqual([], record["claims"]); self.assertEqual({"kind": "Unknown"}, record["rootNativeWitness"])
            for field in ("globalNoOwner", "managedReady", "releaseReady", "devicePASS"):
                self.assertIs(False, record[field])
            self.assertNotIn(b"b" * 32, D.canonical(record))
            self.assertNotIn(base64.b64encode(fixture.secret), D.canonical(record))

    def test_actual_root_entry_reuses_single_controller_worker_and_original_source_loader(self):
        import inspect
        run = inspect.getsource(D.run)
        self.assertEqual(1, run.count("lan.controller_run("))
        self.assertEqual(2, run.count("serve_private_requests("))
        self.assertIn('name="polaris-private-handoff"', run)
        self.assertIn("lan, sources, manifest, source_hash = load_original_sources", run)
        self.assertIn("error.retained_pc_local_foreign = foreign", run)
        self.assertNotIn("waitpid(", run); self.assertNotIn("Popen(", run)


class PcLocalBoundaryTests(unittest.TestCase):
    reject = PcLocalForeignTests.reject

    def test_stop_during_same_raw_pre_constructor_read_does_not_acquire(self):
        fixture = self.reject(InjectedDriver(fault="stop_read_constructor"))
        self.assertTrue(fixture.stop.is_set()); self.assertFalse(fixture.records.closed)
        self.assertEqual([], fixture.sockets); self.assertEqual(6, len(fixture.replies))
        self.assertEqual("NotSent", fixture.output[0]["sent"]["kind"])
        self.assertIn("originalB", fixture.output[0])

    def test_stop_during_same_raw_pre_send_read_retains_exact_handle_without_send(self):
        fixture = self.reject(InjectedDriver(fault="stop_read_send"))
        self.assertTrue(fixture.stop.is_set()); self.assertFalse(fixture.records.closed)
        self.assertEqual(1, len(fixture.sockets)); self.assertEqual(0, fixture.sockets[0].sends)
        self.assertEqual(1, fixture.sockets[0].closes); self.assertEqual(6, len(fixture.replies))
        self.assertIs(fixture.sockets[0], fixture.foreign["tcp"]["custody"].handles[0][1])

    def test_stop_during_final_c_read_does_not_reply_or_commit_observed(self):
        fixture = self.reject(InjectedDriver(fault="stop_read_c"))
        self.assertTrue(fixture.stop.is_set()); self.assertFalse(fixture.records.closed)
        self.assertEqual(7, len(fixture.replies)); self.assertEqual(1, len(fixture.sockets))
        self.assertEqual(33, fixture.output[0]["sent"]["count"])
        self.assertIn("originalC", fixture.output[0]); self.assertEqual(1, fixture.sockets[0].closes)

    def test_c_found_before_wait_expiry_check_cannot_reply_after_delivery_or_read_deadline(self):
        for fault in ("c_delivery_clock", "c_read_clock"):
            with self.subTest(fault=fault):
                fixture = self.reject(InjectedDriver(fault=fault))
                self.assertEqual(7, len(fixture.replies)); self.assertEqual(1, len(fixture.sockets))
                record = fixture.output[0]
                c = L.receiver_record(L.raw_base64(record["originalC"]))
                ready = L.receiver_record(fixture.records.ready_raw)
                self.assertLess(c["observedMonotonicNs"], ready["expiresMonotonicNs"])
                self.assertEqual(33, record["sent"]["count"])

    def test_actual_close_and_guardian_join_cannot_extend_the_original_b_deadline(self):
        for fault in ("close_clock", "join_clock"):
            with self.subTest(fault=fault):
                fixture = self.reject(InjectedDriver(fault=fault))
                self.assertEqual(6, len(fixture.replies)); self.assertEqual(1, len(fixture.sockets))
                record = fixture.output[0]
                self.assertEqual(33, record["sent"]["count"])
                self.assertTrue(record["sendComplete"]); self.assertIn("originalRequest", record)
                self.assertEqual("Closed", record["socketClose"]); self.assertTrue(record["guardianJoined"])
                self.assertEqual(1, fixture.sockets[0].closes)

    def test_original_pre_b_deadline_continues_through_c_without_a_new_window(self):
        fixture = self.reject(InjectedDriver(fault="carry_b_deadline"))
        self.assertEqual(7, len(fixture.replies)); self.assertEqual(1, len(fixture.sockets))
        self.assertEqual(3.001, fixture.foreign["tcp"]["custody"].deadline)
        self.assertEqual(121000000000, L.receiver_record(fixture.records.ready_raw)["expiresMonotonicNs"])
        self.assertIn("originalC", fixture.output[0])

    def test_reply_started_before_boundary_retains_late_write_but_cannot_commit_or_spawn_again(self):
        for fault in ("reply_clock", "reply_stop"):
            with self.subTest(fault=fault):
                fixture = self.reject(InjectedDriver(fault=fault))
                self.assertEqual(8, len(fixture.replies))  # Already started IO cannot be undone.
                self.assertEqual("snapshot", fixture.replies[-1]["action"])
                self.assertEqual(1, len(fixture.sockets)); self.assertEqual(33, fixture.output[0]["sent"]["count"])
                self.assertIn("originalC", fixture.output[0]); self.assertEqual(1, fixture.sockets[0].closes)

    def test_original_identity_changed_during_read_denies_acquisition(self):
        fixture = self.reject(InjectedDriver(fault="identity_read_constructor"))
        self.assertFalse(fixture.stop.is_set()); self.assertFalse(fixture.records.closed)
        self.assertEqual([], fixture.sockets); self.assertEqual(6, len(fixture.replies))

    def test_default_ordinary_reply_has_stop_postcheck_and_no_enabled_phase_deadline(self):
        fixture = InjectedDriver(mode=False, fault="reply_clock"); fixture.run()
        self.assertEqual(25, len(fixture.replies)); self.assertEqual(24, fixture.receiver.snapshots)
        self.assertEqual([], fixture.sockets)
        stopped = InjectedDriver(mode=False, fault="reply_stop")
        with self.assertRaises(D.Unavailable): stopped.run()
        self.assertEqual(8, len(stopped.replies)); self.assertEqual([], stopped.sockets)


if __name__ == "__main__":
    unittest.main()
