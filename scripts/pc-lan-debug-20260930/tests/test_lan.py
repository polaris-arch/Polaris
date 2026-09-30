"""Closed LogicOnly cases. No real socket, core/helper/device or host network effect."""
import base64
import copy
import ctypes
import errno
import importlib.util
import io
import json
import os
import stat
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import types
import unittest
from unittest import mock

ROOT = Path(__file__).absolute().parents[1]
spec = importlib.util.spec_from_file_location("pc_lan_source", ROOT / "lan.py")
lan = importlib.util.module_from_spec(spec)
spec.loader.exec_module(lan)


def example_plan(kind="Synthetic"):
    nonce = "ab" * 16
    return {"schema": "polaris-pc-lan-plan-v1", "scopeSha256": lan.SCOPE_SHA,
            "sourceKind": kind, "echoNonce": nonce,
            "ownerInput": {"kind": kind, "destinationIPv4": "192.168.8.10"},
            "binding": {"runId": "1" * 32, "pcCandidateSha": "2" * 40, "androidCandidateSha": "3" * 40,
                        "receiverSourceSha256": "4" * 64, "androidPackageSha256": "5" * 64,
                        "destinationIPv4": "192.168.8.10", "expectedSenderIPv4": "192.168.8.11",
                        "tcpPort": 55001, "udpPort": 55002, "nonceSha256": lan.sha(nonce.encode()),
                        "lifetimeSeconds": 120, "maxPayloadBytes": 65, "maxConnectionsOrDatagrams": 32}}


class FakeSocket:
    def __init__(self, fail=None):
        self.events, self.address, self.fail, self.closed = [], None, fail, False

    def setblocking(self, value):
        self.events.append(("nonblocking", value))

    def setsockopt(self, *args):
        self.events.append(("exclusive", args))
        if self.fail == "exclusive":
            raise OSError("fixture")

    def bind(self, address):
        self.events.append(("bind", address))
        if self.fail == "bind":
            raise OSError("fixture")
        self.address = address

    def listen(self, backlog):
        self.events.append(("listen", backlog))
        if self.fail == "listen":
            raise OSError("fixture")

    def getsockname(self):
        self.events.append(("getsockname",))
        return self.address

    def close(self):
        self.closed = True
        self.events.append(("close",))


def synthetic_fixture():
    """Every byte here is manufactured; deliberately cannot be device evidence."""
    p = example_plan()
    raw_plan = lan.canonical(p)
    pid = lan.plan_sha(raw_plan)
    raw_events = []
    sockets = [FakeSocket(), FakeSocket()]
    receiver = lan.Receiver(p, pid, "6" * 32, raw_events.append,
                            factory=lambda *args: sockets.pop(0), clock=lambda: 100.0, windows=False)
    receiver.receiver_id = "7" * 32
    with mock.patch.object(lan.secrets, "token_hex", side_effect=["8" * 32, "9" * 32]):
        receiver.setup()
    ready_sha = lan.sha(raw_events[0])
    witnesses, cases = [], []
    attempt = 0
    for cid in lan.CASE_IDS:
        protocol = cid.split("-", 1)[0]
        kind = {"wrong-nonce": "WrongNonce", "foreign-peer": "ForeignPeer",
                "wrong-port-admission": "WrongPortAdmission"}[cid[len(protocol) + 1:]]
        socket_id = next(x["socketInstanceId"] for x in receiver.facts if x["protocol"] == protocol)
        approved = {"address": p["binding"]["destinationIPv4"], "port": p["binding"][protocol + "Port"], "protocol": protocol}
        snapshots, attempts = [], []
        receiver.snapshot()
        snapshots.append(len(raw_events))
        for index in range(3):
            attempt += 1
            attempts.append(attempt)
            case_kind = kind if index == 1 else "Positive"
            expected = p["echoNonce"].encode() + (b"\n" if protocol == "tcp" else b"")
            payload, peer = expected, p["binding"]["expectedSenderIPv4"]
            requested = approved.copy()
            if case_kind == "WrongNonce":
                payload = b"c" * 32 + (b"\n" if protocol == "tcp" else b"")
            if case_kind == "ForeignPeer":
                peer = "192.168.8.12"
            if case_kind == "WrongPortAdmission":
                requested["port"] = p["binding"]["udpPort" if protocol == "tcp" else "tcpPort"]
                sent, returned, outcome = {"kind": "NotSent"}, {"kind": "NotReceived"}, "RejectedBeforeOutbound"
            else:
                receiver.begin_request(protocol)
                receiver.request(protocol, payload, "Complete", peer, lambda data: len(data), lambda n: True, 101)
                sent = {"kind": "Bytes", "count": len(payload), "sha256": lan.sha(payload)}
                returned = sent.copy() if case_kind == "Positive" else {"kind": "NotReceived"}
                outcome = "ExactEcho" if case_kind == "Positive" else "NoEcho"
            witnesses.append({"schema": "polaris-pc-echo-public-witness-v1", "pcRunId": p["binding"]["runId"],
                "pcPlanSha256": pid, "readyReceiptSha256": ready_sha, "receiverInstanceId": receiver.receiver_id,
                "socketInstanceId": socket_id, "protocol": protocol, "caseId": cid, "caseKind": case_kind,
                "attemptSeq": attempt, "requestedTuple": requested, "approvedTuple": approved,
                "sent": sent, "returned": returned, "outcome": outcome,
                "rootNativeWitness": {"kind": "Reference", "sha256": "a" * 64}})
            receiver.snapshot()
            snapshots.append(len(raw_events))
        cases.append({"caseId": cid, "snapshotSeqs": snapshots, "attemptSeqs": attempts})
    receiver.finish("ControllerStop")
    records = [lan.canonical(lan._wrapper("6" * 32, i, "Synthetic", raw)) for i, raw in enumerate(raw_events, 1)]
    records.append(lan.canonical({"schema": "polaris-pc-lan-controller-v1", "role": "Controller", "phase": "ChildWait",
        "controllerInstanceId": "6" * 32, "eventSeq": len(records) + 1, "sourceKind": "Synthetic",
        "childWait": {"state": "Waited", "returnCode": 0}}))
    return {"plan": p, "bundle": lan.bundle_value(raw_plan, records, witnesses, cases)}


def original_receiver(bundle, index):
    wrapper = lan.decode(base64.b64decode(bundle["records"][index]["raw"]))
    return wrapper, lan.decode(base64.b64decode(wrapper["receiverRaw"]))


def rewrite_receiver(bundle, index, child):
    wrapper, _ = original_receiver(bundle, index)
    raw = lan.canonical(child)
    wrapper["receiverRaw"], wrapper["receiverRawSha256"] = base64.b64encode(raw).decode(), lan.sha(raw)
    raw = lan.canonical(wrapper)
    bundle["records"][index] = {"raw": base64.b64encode(raw).decode(), "sha256": lan.sha(raw)}


class CodecTests(unittest.TestCase):
    def test_closed_record_discriminator_and_base64_types_raise_fixed_fault(self):
        fixture = synthetic_fixture()
        wrapper, child = original_receiver(fixture["bundle"], 0)
        for invalid in ([], {}, None, True, 7):
            with self.subTest(invalid=invalid):
                changed = copy.deepcopy(wrapper)
                changed["phase"] = invalid
                with self.assertRaises(lan.Fault):
                    lan.controller_record(lan.canonical(changed))
                changed = copy.deepcopy(child)
                changed["phase"] = invalid
                with self.assertRaises(lan.Fault):
                    lan.receiver_record(lan.canonical(changed))
                changed = copy.deepcopy(wrapper)
                changed["receiverRaw"] = invalid
                with self.assertRaises(lan.Fault):
                    lan.controller_record(lan.canonical(changed))
                with self.assertRaises(lan.Fault):
                    lan.wait_union({"state": invalid})
        for invalid in ("!", "\u00e9", "YWJj===", ""):
            with self.subTest(base64=invalid), self.assertRaises(lan.Fault):
                lan.raw_base64(invalid)

    def test_duplicate_nested_bool_float_nonfinite_unknown_rejected(self):
        for raw in (b'{"a":{"b":1,"b":2}}', b'{"a":1.0}', b'{"a":NaN}', b'\xff', b'{'):
            with self.subTest(raw=raw), self.assertRaises(lan.Fault):
                lan.decode(raw)
        for key, value in (("tcpPort", True), ("maxPayloadBytes", 66), ("udpPort", 55001)):
            p = example_plan()
            p["binding"][key] = value
            with self.assertRaises(lan.Fault):
                lan.plan(p)
        p = example_plan()
        p["ownerInput"]["approved"] = True
        with self.assertRaises(lan.Fault):
            lan.plan(p)

    def test_rfc1918_canonical_only(self):
        for address in ("127.0.0.1", "100.64.1.1", "192.0.0.1", "8.8.8.8", "0.0.0.0", "192.168.01.2", "192.168.1.255", "host"):
            with self.subTest(address=address), self.assertRaises(lan.Fault):
                lan.ipv4(address)

    def test_plan_raw_canonical_hash_and_nonce_binding(self):
        p = example_plan()
        raw = lan.canonical(p)
        self.assertEqual(lan.plan_sha(raw), lan.sha(raw))
        with self.assertRaises(lan.Fault):
            lan.plan_sha(raw + b"\n")
        p["echoNonce"] = "f" * 32
        with self.assertRaises(lan.Fault):
            lan.plan(p)

    def test_source_two_exact_roles_domain_and_no_candidate_self_pin(self):
        items = {"lan.py": b"code\n", "private-windows.ps1": b"shim\n"}
        manifest, actual = lan.source_manifest(items)
        self.assertEqual(actual, lan.sha(b"polaris-pc-lan-execution-source-v1\0" + lan.canonical(manifest)))
        with self.assertRaises(lan.Fault):
            lan.source_manifest({**items, "extra.py": b"extra"})
        changed = items.copy()
        changed["private-windows.ps1"] += b" "
        self.assertNotEqual(actual, lan.source_manifest(changed)[1])

    def test_target_keeps_native_names_absent_and_rejects_secret_in_public(self):
        p = example_plan()
        b = p["binding"]
        target = {"schema": "polaris-pc-echo-target-v1", "pcRunId": b["runId"], "pcPlanSha256": "1" * 64,
            "readyReceiptSha256": "2" * 64, "receiverInstanceId": "3" * 32, "tcpSocketInstanceId": "4" * 32,
            "udpSocketInstanceId": "5" * 32, **{k: b[k] for k in b if k not in
                ("runId", "nonceSha256", "maxPayloadBytes", "maxConnectionsOrDatagrams")},
            "echoNonce": p["echoNonce"], "echoNonceSha256": b["nonceSha256"], "maxRequests": 32}
        self.assertEqual(lan.target(target), target)
        target["password"] = "secret"
        with self.assertRaises(lan.Fault):
            lan.target(target)
        witness = synthetic_fixture()["bundle"]["publicWitnesses"][0]
        witness["echoNonce"] = p["echoNonce"]
        with self.assertRaises(lan.Fault):
            lan.public_witness(witness)


class WireTests(unittest.TestCase):
    def test_first_lf_segmentation_extra_and_boundary(self):
        for n in (32, 64):
            frame = b"a" * n + b"\n"
            for chunks in ([frame + b"extra"], [frame, b"extra"], [frame[:3], frame[3:-1], b"\nextra"]):
                self.assertEqual(lan.first_lf_frame(chunks), (frame, "Complete"))
        self.assertEqual(lan.first_lf_frame([b"a" * 65, b"\n"])[1], "NoLF")
        self.assertEqual(lan.first_lf_frame([b"short", b""])[1], "EarlyEOF")

    def test_slow_segments_share_one_absolute_read_deadline(self):
        now = [0.0]
        waits = []
        def recv(_):
            now[0] += .3
            return b"a"
        def wait(seconds):
            waits.append(seconds)
            return True
        frame, result = lan.tcp_read(recv, wait, lambda: now[0], 1.0)
        self.assertEqual(result, "ReadTimeout")
        self.assertEqual(len(frame), 4)
        self.assertLess(waits[-1], waits[0])

    def test_partial_echo_preserves_match_and_failed_write(self):
        events = []
        r = lan.Receiver(example_plan(), "a" * 64, "b" * 32, events.append, clock=lambda: 1)
        r.facts = [{"protocol": "tcp", "socketInstanceId": "c" * 32}]
        sends = [2]
        def send(_):
            if sends:
                return sends.pop()
            raise OSError("fixture")
        r.begin_request("tcp")
        r.request("tcp", b"ab" * 16 + b"\n", "Complete", "192.168.8.11", send, lambda n: True, 2)
        self.assertEqual((r.counters["tcp"]["matched"], r.counters["tcp"]["echoWritten"], r.counters["tcp"]["writeFailed"]), (1, 0, 1))

    def test_foreign_and_malformed_each_consume_one_shared_budget(self):
        r = lan.Receiver(example_plan(), "a" * 64, "b" * 32, lambda _: None, clock=lambda: 1)
        r.facts = [{"protocol": p, "socketInstanceId": "c" * 32} for p in ("tcp", "udp")]
        for i in range(32):
            p = "tcp" if i % 2 else "udp"
            r.begin_request(p)
            r.request(p, b"wrong", "Complete", "192.168.8.12", lambda _: self.fail("foreign echoed"), lambda n: True, 2)
        self.assertEqual(r.requests, 32)
        self.assertEqual(sum(x["foreignPeer"] for x in r.counters.values()), 32)
        self.assertEqual(sum(x["wrongNonce"] for x in r.counters.values()), 0)
        with self.assertRaises(lan.Fault):
            r.begin_request("udp")


class CustodyTests(unittest.TestCase):
    def test_receiver_stop_does_not_select_closed_sockets_or_consume_later_controls(self):
        events, remaining = [], [FakeSocket(), FakeSocket()]
        receiver = lan.Receiver(example_plan(), "a" * 64, "b" * 32, events.append,
            factory=lambda *args: remaining.pop(0), windows=False)
        controls = types.SimpleNamespace(eof=False, drain=lambda: [b'{"command":"stop"}', b'{"command":"snapshot"}'])
        output = types.SimpleNamespace(pending=bytearray(), pump=lambda: None)
        with mock.patch.object(lan.select, "select", side_effect=AssertionError("selected after close")):
            lan.receiver_loop(receiver, controls, output)
        records = [lan.receiver_record(raw) for raw in events]
        self.assertEqual([x["phase"] for x in records], ["Ready", "Closed"])
        self.assertTrue(records[-1]["allHandlesClosed"])
        self.assertEqual(records[-1]["stopReason"], "ControllerStop")

    def test_parallel_release_and_guardian_close_original_object_once(self):
        entered, finish = threading.Event(), threading.Event()
        calls = []
        class HeldClose:
            def close(self):
                calls.append("close")
                entered.set()
                finish.wait(2)
        handle = HeldClose()
        custody = lan.SocketCustody(time.monotonic() + 10)
        custody.publish("tcp", handle)
        worker = threading.Thread(target=lambda: custody.release("tcp", handle))
        worker.start()
        try:
            self.assertTrue(entered.wait(1))
            custody.close("ControllerStop")
            self.assertFalse(custody.closed.is_set())
            self.assertEqual(calls, ["close"])
        finally:
            finish.set()
            worker.join(2)
        self.assertTrue(custody.closed.is_set())
        self.assertEqual(custody.results, {"tcp": "Closed"})

    def test_close_failure_is_retained_unknown_and_never_replaced_by_success(self):
        handle = types.SimpleNamespace(close=mock.Mock(side_effect=OSError("fixture")))
        custody = lan.SocketCustody(time.monotonic() + 10)
        custody.publish("tcp", handle)
        custody.close("ControllerStop")
        custody.release("tcp", handle)
        self.assertEqual(custody.results, {"tcp": "Unknown"})
        self.assertIs(custody.handles[0][1], handle)
        handle.close.assert_called_once()

    def test_controller_setup_and_output_failure_use_eof_same_waiter(self):
        class Pipe:
            def __init__(self, fd):
                self.fd, self.closed = fd, False
            def fileno(self):
                return self.fd
            def close(self):
                self.closed = True
        class Child:
            def __init__(self, *args, **kwargs):
                self.stdin, self.stdout, self.pid, self.returncode = Pipe(701), Pipe(702), 703, None
        class Input:
            def __init__(self, *args):
                self.pending, self.eof = bytearray(), True
            def drain(self):
                return []
        for fail in ("pipe-setup", "output", "interrupt"):
            with self.subTest(fail=fail):
                calls, children = [], []
                actual_init = Child.__init__
                def init(child, *args, **kwargs):
                    actual_init(child, *args, **kwargs)
                    children.append(child)
                    if fail == "interrupt":
                        lan.signal.getsignal(lan.signal.SIGINT)(lan.signal.SIGINT, None)
                def set_blocking(fd, value):
                    if fail == "pipe-setup" and fd == 701:
                        raise OSError("fixture")
                def waited(pid, flags):
                    calls.append(pid)
                    return pid, 7 << 8
                def writer(fd, raw):
                    if fail == "output":
                        raise BrokenPipeError("fixture")
                    return len(raw)
                sources = {path: b"fixture source" for _, path in lan.SOURCE_ITEMS}
                manifest, digest = lan.source_manifest(sources)
                p = example_plan("OperatorDeclared")
                p["binding"]["receiverSourceSha256"] = digest
                original_waiter = lan.OriginalWaiter
                with mock.patch.object(Child, "__init__", init), mock.patch.object(lan, "OwnedPopen", Child), \
                     mock.patch.object(lan.os, "set_blocking", set_blocking), mock.patch.object(lan, "LineInput", Input), \
                     mock.patch.object(lan.os, "fstat", return_value=types.SimpleNamespace(st_mode=stat.S_IFIFO)), \
                     mock.patch.object(lan, "OriginalWaiter", lambda child: original_waiter(child, waitpid=waited)):
                    raw = lan.controller_run(lan.canonical(p), sources, manifest, lan.OutputQueue(700, writer), 704)
                self.assertEqual(calls, [703])
                self.assertTrue(children[0].stdin.closed)
                self.assertTrue(children[0].stdout.closed)
                self.assertFalse(lan.UNRESOLVED_CHILDREN)
                records = [lan.controller_record(item) for item in raw]
                if fail == "interrupt":
                    self.assertTrue(any(x["phase"] == "StopRequested" and x["reason"] == "ControllerInterrupted"
                                        for x in records))
                else:
                    self.assertTrue(any(x["phase"] == "Unknown" for x in records))
                self.assertEqual(records[-1]["childWait"], {"state": "Waited", "returnCode": 7})

    def test_controller_rejects_blocking_regular_record_sink_before_child(self):
        sources = {path: b"fixture source" for _, path in lan.SOURCE_ITEMS}
        manifest, digest = lan.source_manifest(sources)
        p = example_plan("OperatorDeclared")
        p["binding"]["receiverSourceSha256"] = digest
        with mock.patch.object(lan.os, "fstat", return_value=types.SimpleNamespace(st_mode=stat.S_IFREG)), \
             mock.patch.object(lan, "OwnedPopen", side_effect=AssertionError("spawned")):
            with self.assertRaises(lan.Fault) as error:
                lan.controller_run(lan.canonical(p), sources, manifest, lan.OutputQueue(700), 704)
        self.assertEqual(error.exception.code, "RecordPipeRequired")

    def test_tcp_listen_udp_bind_and_tuple_before_ready(self):
        handles, events = [FakeSocket(), FakeSocket()], []
        remaining = handles.copy()
        r = lan.Receiver(example_plan(), "a" * 64, "b" * 32, events.append,
                         factory=lambda *args: remaining.pop(0), windows=False)
        try:
            r.setup()
            self.assertIn(("listen", 4), handles[0].events)
            self.assertNotIn(("listen", 4), handles[1].events)
            ready = lan.receiver_record(events[0])
            self.assertTrue(ready["sockets"][0]["tcpListening"])
        finally:
            r.finish("ControllerStop")

    def test_second_bind_or_listen_failure_never_ready_and_closes_owned(self):
        for failure in ("bind", "listen"):
            handles = [FakeSocket(failure if failure == "listen" else None), FakeSocket("bind" if failure == "bind" else None)]
            remaining, events = handles.copy(), []
            r = lan.Receiver(example_plan(), "a" * 64, "b" * 32, events.append,
                             factory=lambda *args: remaining.pop(0), windows=False)
            with self.assertRaises(OSError):
                r.setup()
            self.assertFalse(events)
            self.assertTrue(handles[0].closed)
            if failure == "bind":
                self.assertTrue(handles[1].closed)

    def test_missing_or_failed_windows_exclusive_is_before_bind(self):
        if not hasattr(lan.socket, "SO_EXCLUSIVEADDRUSE"):
            r = lan.Receiver(example_plan(), "a" * 64, "b" * 32, lambda _: None,
                             factory=lambda *args: self.fail("created socket"), windows=True)
            with self.assertRaises(lan.Fault):
                r.setup()
        handle = FakeSocket("exclusive")
        with mock.patch.object(lan.socket, "SO_EXCLUSIVEADDRUSE", 0xFFFF, create=True):
            r = lan.Receiver(example_plan(), "a" * 64, "b" * 32, lambda _: None, factory=lambda *args: handle, windows=True)
            with self.assertRaises(OSError):
                r.setup()
        self.assertFalse(any(e[0] == "bind" for e in handle.events))
        self.assertTrue(handle.closed)

    def test_guardian_closes_even_when_record_writer_is_blocked(self):
        custody = lan.SocketCustody(time.monotonic() + .08)
        handle = FakeSocket()
        custody.publish("tcp", handle)
        gate, writing = threading.Event(), threading.Event()
        def write(_, data):
            writing.set()
            gate.wait(2)
            return len(data)
        output = lan.OutputQueue(999, write)
        output.put(b'{}')
        worker = threading.Thread(target=output.pump)
        guardian = threading.Thread(target=custody.guardian)
        worker.start()
        guardian.start()
        try:
            self.assertTrue(writing.wait(1))
            self.assertTrue(custody.closed.wait(1))
            self.assertTrue(handle.closed)
            self.assertTrue(worker.is_alive())
        finally:
            gate.set()
            worker.join(2)
            guardian.join(2)

    def test_pipe_backpressure_is_bounded_without_dropping_pending_bytes(self):
        def blocked(*args):
            raise BlockingIOError(errno.EAGAIN, "fixture")
        output = lan.OutputQueue(999, blocked)
        output.put(b'{}')
        original = bytes(output.pending)
        output.pump()
        self.assertEqual(bytes(output.pending), original)
        with self.assertRaises(lan.Fault):
            for _ in range(lan.MAX_EVENTS):
                output.put(b'{}')

    def test_wait_unknown_is_not_exit_and_echild_never_retries(self):
        child = types.SimpleNamespace(pid=123, returncode=None)
        calls = []
        def wait(pid, flags):
            calls.append(pid)
            raise ChildProcessError(errno.ECHILD, "fixture")
        waiter = lan.OriginalWaiter(child, waitpid=wait)
        self.assertEqual(waiter.observe()["state"], "Unknown")
        self.assertEqual(waiter.observe()["error"], "LostWaitOwnership")
        self.assertEqual(calls, [123])
        self.assertIsNone(child.returncode)

    def test_generic_wait_error_can_retry_same_object_without_clearing_prior_unknown(self):
        child = types.SimpleNamespace(pid=123, returncode=None)
        wait = mock.Mock(side_effect=[OSError(errno.EINTR, "fixture"), (123, 7 << 8)])
        waiter = lan.OriginalWaiter(child, waitpid=wait)
        self.assertEqual(waiter.observe(), {"state": "Unknown", "error": "NativeWaitUnknown"})
        self.assertIsNone(child.returncode)
        self.assertEqual(waiter.observe(), {"state": "Waited", "returnCode": 7})
        self.assertEqual(wait.call_count, 2)
        self.assertEqual(wait.call_args.args[0], 123)

    def test_windows_timeout_does_not_read_code_then_same_handle_success(self):
        child = types.SimpleNamespace(_handle=object(), returncode=None)
        api = types.SimpleNamespace(WaitForSingleObject=mock.Mock(side_effect=[258, 0]), GetExitCodeProcess=mock.Mock(return_value=9))
        waiter = lan.OriginalWaiter(child, win=api)
        self.assertEqual(waiter.observe(), {"state": "Pending"})
        api.GetExitCodeProcess.assert_not_called()
        self.assertEqual(waiter.observe(), {"state": "Waited", "returnCode": 9})
        self.assertIs(api.WaitForSingleObject.call_args.args[0], child._handle)

    @unittest.skipIf(os.name == "nt", "Unix native original wait fixture")
    def test_actual_no_network_child_nonzero_and_external_reap_echild(self):
        for external in (False, True):
            child = lan.OwnedPopen([sys.executable, "-I", "-S", "-c", "raise SystemExit(7)"], stdin=subprocess.DEVNULL,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            waiter = lan.OriginalWaiter(child)
            if external:
                os.waitpid(child.pid, 0)  # Fixture owner consumes it; no live orphan is leaked.
                self.assertEqual(waiter.observe()["error"], "LostWaitOwnership")
                self.assertIsNone(child.returncode)
                self.assertEqual(waiter.observe()["state"], "Unknown")
            else:
                end = time.monotonic() + 3
                value = waiter.observe()
                while value["state"] == "Pending" and time.monotonic() < end:
                    time.sleep(.005)
                    value = waiter.observe()
                self.assertEqual(value, {"state": "Waited", "returnCode": 7})
            with self.assertRaises(lan.Fault):
                child.poll()


class FileTests(unittest.TestCase):
    def test_windows_actual_security_validator_rejects_inherited_broad_or_unprotected(self):
        for changed in (None, "inherited", "broad", "unprotected", "owner", "additional-ace"):
            with self.subTest(changed=changed):
                api = lan.WinFiles.__new__(lan.WinFiles)
                api.w, api.sid = types.SimpleNamespace(DWORD=ctypes.c_uint32), 555
                acl = ctypes.create_string_buffer(b'\x02\0\x08\0' +
                    (2 if changed == "additional-ace" else 1).to_bytes(2, "little") + b'\0\0')
                ace = ctypes.create_string_buffer(bytes([0, 16 if changed == "inherited" else 0, 24, 0]) +
                    (0x1F01FF).to_bytes(4, "little") + b'fixture-sid')
                def info(handle, _, flags, owner, group, dacl, sacl, descriptor):
                    self.assertEqual(handle, 111)
                    ctypes.cast(owner, ctypes.POINTER(ctypes.c_void_p))[0] = 777
                    ctypes.cast(dacl, ctypes.POINTER(ctypes.c_void_p))[0] = ctypes.addressof(acl)
                    ctypes.cast(descriptor, ctypes.POINTER(ctypes.c_void_p))[0] = 888
                    return 0
                def control(descriptor, output, revision):
                    ctypes.cast(output, ctypes.POINTER(ctypes.c_ushort))[0] = 0 if changed == "unprotected" else 0x1000
                    return 1
                def get_ace(pointer, index, output):
                    ctypes.cast(output, ctypes.POINTER(ctypes.c_void_p))[0] = ctypes.addressof(ace)
                    return 1
                def same_sid(left, right):
                    return (changed != "owner") if isinstance(left, ctypes.c_void_p) else changed != "broad"
                api.a = types.SimpleNamespace(GetSecurityInfo=info, GetSecurityDescriptorControl=control,
                    GetAce=get_ace, EqualSid=same_sid)
                api.k = types.SimpleNamespace(LocalFree=mock.Mock())
                if changed is None:
                    api.security(111)
                else:
                    with self.assertRaises(lan.Fault):
                        api.security(111)
                api.k.LocalFree.assert_called_once()

    def test_failed_windows_close_preserves_handle_and_attempts_all(self):
        api = lan.WinFiles.__new__(lan.WinFiles)
        api.k = types.SimpleNamespace(CloseHandle=mock.Mock(side_effect=[False, True]))
        before = len(lan.UNRESOLVED_FILE_HANDLES)
        try:
            with self.assertRaises(lan.Fault) as error:
                api.close_all([11, 22])
            self.assertEqual(error.exception.code, "WindowsHandleCloseUnknown")
            self.assertEqual(api.k.CloseHandle.call_count, 2)
            self.assertEqual(lan.UNRESOLVED_FILE_HANDLES[before:], [(api, 11)])
        finally:
            del lan.UNRESOLVED_FILE_HANDLES[before:]

    @unittest.skipIf(os.name == "nt", "POSIX file fixture")
    def test_open_fd_regular_private_create_new_no_symlinks_or_fifo(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            lan.create_directory(root / "private")
            path = root / "private" / "input"
            lan.create_file(path, b'{}')
            self.assertEqual(lan.read_file(path), b'{}')
            with self.assertRaises(FileExistsError):
                lan.create_file(path, b'overwritten')
            os.symlink(path, root / "private" / "link")
            with self.assertRaises(OSError):
                lan.read_file(root / "private" / "link")
            os.mkfifo(root / "private" / "fifo", 0o600)
            with self.assertRaises(lan.Fault):
                lan.read_file(root / "private" / "fifo")
            os.chmod(path, 0o644)
            with self.assertRaises(lan.Fault):
                lan.read_file(path)

    def test_windows_native_private_validation_precedes_read_or_write(self):
        api = lan.WinFiles.__new__(lan.WinFiles)
        api.w = types.SimpleNamespace(DWORD=ctypes.c_uint32)
        api._parents = mock.Mock(return_value=([11], "owned"))
        api._open = mock.Mock(return_value=22)
        api.metadata = mock.Mock(return_value=(1, 2, 3, 0, 2, 4, 5))
        api.security = mock.Mock(side_effect=lan.Fault("WindowsPrivateDACL"))
        api._descriptor = mock.Mock(return_value=ctypes.c_void_p(33))
        api.k = types.SimpleNamespace(ReadFile=mock.Mock(), WriteFile=mock.Mock(), CloseHandle=mock.Mock(return_value=True), LocalFree=mock.Mock())
        with self.assertRaises(lan.Fault):
            api.read("owned", 1024, True)
        api.k.ReadFile.assert_not_called()
        with self.assertRaises(lan.Fault):
            api.create("owned", b'secret')
        api.k.WriteFile.assert_not_called()
        self.assertIs(api._open.call_args.kwargs["descriptor"], api._descriptor.return_value)


class VerificationTests(unittest.TestCase):
    def setUp(self):
        self.fixture = synthetic_fixture()
        self.raw_plan = lan.canonical(self.fixture["plan"])
        self.bundle = self.fixture["bundle"]

    def test_request_completion_before_at_and_after_original_lease_end(self):
        _, ready = original_receiver(self.bundle, 0)
        for offset, expected in ((-1, "LogicOnly"), (0, "Invalid"), (1, "Invalid")):
            with self.subTest(offset=offset):
                data, began = copy.deepcopy(self.bundle), False
                for index in range(len(data["records"]) - 1):
                    _, child = original_receiver(data, index)
                    began |= child["phase"] == "Request"
                    if began:
                        child["observedMonotonicNs"] = ready["expiresMonotonicNs"] + offset
                        rewrite_receiver(data, index, child)
                self.assertEqual(lan.verify(self.raw_plan, data)["verdict"], expected)

    def test_each_case_snapshot_at_or_after_expiry_cannot_authorize_business(self):
        _, ready = original_receiver(self.bundle, 0)
        for position in range(4):
            for offset in (0, 1):
                with self.subTest(snapshot=position, offset=offset):
                    data = copy.deepcopy(self.bundle)
                    first = data["cases"][-1]["snapshotSeqs"][position] - 1
                    for index in range(first, len(data["records"]) - 1):
                        _, child = original_receiver(data, index)
                        child["observedMonotonicNs"] = ready["expiresMonotonicNs"] + offset
                        rewrite_receiver(data, index, child)
                    self.assertEqual(lan.verify(self.raw_plan, data)["verdict"], "Invalid")

    def test_missing_request_or_case_observation_time_remains_incomplete(self):
        for phase in ("Request", "Counters"):
            with self.subTest(phase=phase):
                data = copy.deepcopy(self.bundle)
                for index in range(len(data["records"]) - 1):
                    _, child = original_receiver(data, index)
                    if child["phase"] == phase:
                        del child["observedMonotonicNs"]
                        rewrite_receiver(data, index, child)
                        break
                result = lan.verify(self.raw_plan, data)
                self.assertEqual(result["verdict"], "Incomplete")
                self.assertIn("MissingBusinessTime", result["reasons"])

    def test_late_original_close_wait_and_transport_tail_keep_valid_business(self):
        data = copy.deepcopy(self.bundle)
        _, ready = original_receiver(data, 0)
        _, closed = original_receiver(data, len(data["records"]) - 2)
        closed["observedMonotonicNs"] = ready["expiresMonotonicNs"] + 1000
        rewrite_receiver(data, len(data["records"]) - 2, closed)
        # The original Waited bytes and their pipe delivery have no business-window
        # timestamp. Neither late close nor late tail transport renews a request lease.
        self.assertEqual(lan.verify(self.raw_plan, data)["verdict"], "LogicOnly")
        self.assertEqual(lan.controller_record(base64.b64decode(data["records"][-1]["raw"]))["childWait"],
                         {"state": "Waited", "returnCode": 0})

    def test_late_diagnostic_counter_is_not_a_case_window_or_close_proof(self):
        _, ready = original_receiver(self.bundle, 0)
        _, diagnostic = original_receiver(self.bundle, 1)
        _, closed = original_receiver(self.bundle, len(self.bundle["records"]) - 2)
        diagnostic["observedMonotonicNs"] = ready["expiresMonotonicNs"] + 1
        closed["observedMonotonicNs"] = ready["expiresMonotonicNs"] + 2
        closed["counters"] = lan.empty_counters()
        records = []
        for index, child in enumerate((ready, diagnostic, closed), 1):
            child["receiverSeq"] = index
            records.append(lan.canonical(lan._wrapper("6" * 32, index, "Synthetic", lan.canonical(child))))
        records.append(lan.canonical({"schema": "polaris-pc-lan-controller-v1", "role": "Controller", "phase": "ChildWait",
            "controllerInstanceId": "6" * 32, "eventSeq": 4, "sourceKind": "Synthetic",
            "childWait": {"state": "Waited", "returnCode": 0}}))
        data = lan.bundle_value(self.raw_plan, records, [], [])
        self.assertEqual(lan.verify(self.raw_plan, data), lan.claims("Incomplete", ["MissingBatchCases"]))

    def test_request_own_counter_increment_cannot_be_missing_wrong_protocol_or_extra_category(self):
        for problem in ("missing", "wrong-protocol", "extra-category"):
            with self.subTest(problem=problem):
                data = copy.deepcopy(self.bundle)
                index = next(index for index in range(len(data["records"]) - 1)
                             if original_receiver(data, index)[1]["phase"] == "Request")
                _, child = original_receiver(data, index)
                if problem == "missing":
                    child["counters"] = lan.empty_counters()
                elif problem == "wrong-protocol":
                    child["counters"]["udp"] = child["counters"]["tcp"].copy()
                    child["counters"]["tcp"] = lan.empty_counters()["tcp"]
                else:
                    child["counters"]["tcp"]["total"] += 1
                    child["counters"]["tcp"]["wrongNonce"] += 1
                rewrite_receiver(data, index, child)
                self.assertEqual(lan.verify(self.raw_plan, data)["verdict"], "Invalid")

    def test_request_echo_state_must_explain_exact_written_or_failed_counter(self):
        index = next(index for index in range(len(self.bundle["records"]) - 1)
                     if original_receiver(self.bundle, index)[1]["phase"] == "Request")
        for problem in ("false-complete", "false-failed"):
            with self.subTest(problem=problem):
                data = copy.deepcopy(self.bundle)
                _, child = original_receiver(data, index)
                if problem == "false-complete":
                    child["counters"]["tcp"]["echoWritten"] = 0
                    child["counters"]["tcp"]["writeFailed"] = 1
                else:
                    child["request"]["echoState"] = "Failed"
                rewrite_receiver(data, index, child)
                self.assertEqual(lan.verify(self.raw_plan, data)["verdict"], "Invalid")
        _, actual_failed = original_receiver(self.bundle, index)
        actual_failed["request"]["echoState"] = "Failed"
        actual_failed["counters"]["tcp"]["echoWritten"] = 0
        actual_failed["counters"]["tcp"]["writeFailed"] = 1
        self.assertFalse(lan.counter_transition(lan.empty_counters(), lan.receiver_record(lan.canonical(actual_failed))))

    def test_counter_snapshot_cannot_manufacture_a_missing_request_result(self):
        data = copy.deepcopy(self.bundle)
        _, counter = original_receiver(data, 1)
        counter["counters"]["tcp"]["total"] = 1
        counter["counters"]["tcp"]["matched"] = 1
        counter["counters"]["tcp"]["echoWritten"] = 1
        rewrite_receiver(data, 1, counter)
        self.assertEqual(lan.verify(self.raw_plan, data)["verdict"], "Invalid")

    def test_actual_accept_before_error_retains_pending_unknown_without_completed_request(self):
        events, handles = [], [FakeSocket(), FakeSocket()]
        receiver = lan.Receiver(self.fixture["plan"], lan.plan_sha(self.raw_plan), "6" * 32, events.append,
            factory=lambda *args: handles.pop(0), clock=lambda: 100.0, windows=False)
        receiver.setup()
        receiver.begin_request("tcp")
        receiver.record("Unknown", error="ReceiverIOUnknown")
        receiver.finish("ReceiverIOUnknown")
        records = [lan.canonical(lan._wrapper("6" * 32, index, "Synthetic", raw))
                   for index, raw in enumerate(events, 1)]
        records.append(lan.canonical({"schema": "polaris-pc-lan-controller-v1", "role": "Controller", "phase": "ChildWait",
            "controllerInstanceId": "6" * 32, "eventSeq": len(records) + 1, "sourceKind": "Synthetic",
            "childWait": {"state": "Waited", "returnCode": 0}}))
        result = lan.verify(self.raw_plan, lan.bundle_value(self.raw_plan, records, [], []))
        self.assertEqual(result["verdict"], "Incomplete")
        self.assertIn("UnfinishedAcceptedRequest", result["reasons"])
        self.assertIn("ReceiverUnknown", result["reasons"])
        self.assertEqual(lan.receiver_record(events[-1])["counters"]["tcp"]["pending"], 1)
        self.assertFalse(any(lan.receiver_record(raw)["phase"] == "Request" for raw in events))

    def test_complete_synthetic_only_logic_and_handfill_cannot_device_pass(self):
        result = lan.verify(self.raw_plan, self.bundle)
        self.assertEqual(result["verdict"], "LogicOnly")
        self.assertEqual(result["claims"], [])
        self.assertFalse(result["outboundReady"])
        # Change every self-reported origin consistently. It still cannot authenticate itself.
        p = copy.deepcopy(self.fixture["plan"])
        p["sourceKind"] = p["ownerInput"]["kind"] = "OperatorDeclared"
        raw = lan.canonical(p)
        data = copy.deepcopy(self.bundle)
        data["sourceKind"], data["pcPlanSha256"] = "OperatorDeclared", lan.plan_sha(raw)
        ready_sha = None
        for entry in data["records"]:
            item = lan.decode(base64.b64decode(entry["raw"]))
            item["sourceKind"] = "OperatorDeclared"
            if item["phase"] == "ReceiverRecord":
                child = lan.decode(base64.b64decode(item["receiverRaw"]))
                child["sourceKind"], child["pcPlanSha256"] = "OperatorDeclared", data["pcPlanSha256"]
                child_raw = lan.canonical(child)
                item["receiverRaw"], item["receiverRawSha256"] = base64.b64encode(child_raw).decode(), lan.sha(child_raw)
                if child["phase"] == "Ready":
                    ready_sha = lan.sha(child_raw)
            encoded = lan.canonical(item)
            entry["raw"], entry["sha256"] = base64.b64encode(encoded).decode(), lan.sha(encoded)
        for witness in data["publicWitnesses"]:
            witness["pcPlanSha256"], witness["readyReceiptSha256"] = data["pcPlanSha256"], ready_sha
        result = lan.verify(raw, data)
        self.assertEqual(result["verdict"], "EvidenceConsistentUnverified")
        self.assertFalse(result["networkExact"])

    def test_missing_old_or_wrong_original_rejected(self):
        data = copy.deepcopy(self.bundle)
        data["records"].pop()
        self.assertEqual(lan.verify(self.raw_plan, data)["verdict"], "Incomplete")
        data = copy.deepcopy(self.bundle)
        data["publicWitnesses"][0]["readyReceiptSha256"] = "0" * 64
        self.assertEqual(lan.verify(self.raw_plan, data)["verdict"], "Invalid")
        data = copy.deepcopy(self.bundle)
        data["records"][0]["sha256"] = "0" * 64
        self.assertEqual(lan.verify(self.raw_plan, data)["verdict"], "Invalid")

    def test_unknown_native_reference_foreign_unobserved_and_wrongport_send(self):
        data = copy.deepcopy(self.bundle)
        data["publicWitnesses"][0]["rootNativeWitness"] = {"kind": "Unknown"}
        self.assertEqual(lan.verify(self.raw_plan, data)["verdict"], "Incomplete")
        data = copy.deepcopy(self.bundle)
        data["cases"].pop(1)
        data["publicWitnesses"] = [x for x in data["publicWitnesses"] if x["caseId"] != "tcp-foreign-peer"]
        self.assertEqual(lan.verify(self.raw_plan, data)["verdict"], "Incomplete")
        data = copy.deepcopy(self.bundle)
        probe = next(x for x in data["publicWitnesses"] if x["caseKind"] == "WrongPortAdmission")
        probe["sent"] = {"kind": "Bytes", "count": 0, "sha256": lan.sha(b"")}
        self.assertEqual(lan.verify(self.raw_plan, data)["verdict"], "Invalid")

    def test_ready_not_wait_roles_sequence_and_closed_unknown(self):
        data = copy.deepcopy(self.bundle)
        item = lan.decode(base64.b64decode(data["records"][-1]["raw"]))
        item["childWait"] = {"state": "Unknown", "error": "LostWaitOwnership"}
        encoded = lan.canonical(item)
        data["records"][-1] = {"raw": base64.b64encode(encoded).decode(), "sha256": lan.sha(encoded)}
        self.assertEqual(lan.verify(self.raw_plan, data)["verdict"], "Incomplete")
        data = copy.deepcopy(self.bundle)
        data["records"][1], data["records"][2] = data["records"][2], data["records"][1]
        self.assertEqual(lan.verify(self.raw_plan, data)["verdict"], "Invalid")

    def test_complete_matrix_cannot_ignore_noise_after_last_case(self):
        data = copy.deepcopy(self.bundle)
        entry = data["records"][-2]
        item = lan.decode(base64.b64decode(entry["raw"]))
        child = lan.decode(base64.b64decode(item["receiverRaw"]))
        child["counters"]["tcp"]["total"] += 1
        child["counters"]["tcp"]["foreignPeer"] += 1
        encoded = lan.canonical(child)
        item["receiverRaw"], item["receiverRawSha256"] = base64.b64encode(encoded).decode(), lan.sha(encoded)
        encoded = lan.canonical(item)
        data["records"][-2] = {"raw": base64.b64encode(encoded).decode(), "sha256": lan.sha(encoded)}
        self.assertEqual(lan.verify(self.raw_plan, data)["verdict"], "Invalid")

    def test_receiver_cannot_mint_wait_and_unknown_cannot_be_washed(self):
        raw = base64.b64decode(self.bundle["records"][0]["raw"])
        wrapper = lan.decode(raw)
        child = lan.decode(base64.b64decode(wrapper["receiverRaw"]))
        child["childWait"] = {"state": "Waited", "returnCode": 0}
        with self.assertRaises(lan.Fault):
            lan.receiver_record(lan.canonical(child))
        data = copy.deepcopy(self.bundle)
        unknown = {"schema": "polaris-pc-lan-controller-v1", "role": "Controller", "phase": "Unknown",
            "controllerInstanceId": "6" * 32, "eventSeq": len(data["records"]) + 1,
            "sourceKind": "Synthetic", "error": "ControllerIOUnknown"}
        raw = lan.canonical(unknown)
        data["records"].append({"raw": base64.b64encode(raw).decode(), "sha256": lan.sha(raw)})
        self.assertEqual(lan.verify(self.raw_plan, data)["verdict"], "Incomplete")

    def test_generic_native_error_later_wait_is_incomplete_echild_later_wait_invalid(self):
        for error, verdict in (("NativeWaitUnknown", "Incomplete"), ("LostWaitOwnership", "Invalid")):
            with self.subTest(error=error):
                data = copy.deepcopy(self.bundle)
                position = len(data["records"]) - 1
                wait = lan.decode(base64.b64decode(data["records"][-1]["raw"]))
                unknown = copy.deepcopy(wait)
                unknown["childWait"] = {"state": "Unknown", "error": error}
                raw = lan.canonical(unknown)
                data["records"].insert(position, {"raw": base64.b64encode(raw).decode(), "sha256": lan.sha(raw)})
                wait["eventSeq"] += 1
                raw = lan.canonical(wait)
                data["records"][-1] = {"raw": base64.b64encode(raw).decode(), "sha256": lan.sha(raw)}
                self.assertEqual(lan.verify(self.raw_plan, data)["verdict"], verdict)

    def test_public_serve_denies_missing_coordination_not_caller_bool(self):
        with self.assertRaises(lan.Fault) as context:
            lan.root_coordinated_execution({"approved": True, "actualInstalled": True})
        self.assertEqual(context.exception.code, "ExecutionNotFrozen")

    def test_frozen_fixture_matches_codec_and_no_raw_nonce_public(self):
        frozen = lan.decode((ROOT / "fixtures" / "synthetic-controller.json").read_bytes(), 4 * lan.MAX_OUTPUT)
        self.assertEqual(lan.verify(lan.canonical(frozen["plan"]), frozen["bundle"])["verdict"], "LogicOnly")
        self.assertNotIn(frozen["plan"]["echoNonce"], lan.canonical(frozen["bundle"]).decode())

    def test_private_target_bridge_uses_original_ready_bytes_without_current_claim(self):
        first = lan.decode(base64.b64decode(self.bundle["records"][0]["raw"]))
        raw_ready = base64.b64decode(first["receiverRaw"])
        value = lan.target_from_original_ready(self.raw_plan, raw_ready)
        self.assertEqual(value["readyReceiptSha256"], lan.sha(raw_ready))
        self.assertEqual(value["echoNonce"], self.fixture["plan"]["echoNonce"])
        self.assertNotIn("currentReady", value)
        self.assertNotIn("nativeScope", value)
        self.assertNotIn("executionReady", value)


class CLITests(unittest.TestCase):
    def test_collect_malformed_phase_or_receiver_bytes_is_typed_invalid_without_bundle_or_echo(self):
        fixture = synthetic_fixture()
        wrapper, child = original_receiver(fixture["bundle"], 0)
        marker = "private-input-secret-marker"
        paths = {key: "/private-operator-path/" + marker + "/" + key
                 for key in ("plan", "records", "witnesses", "cases", "output")}
        for problem, code in (("phase-list", "InvalidDiscriminator"), ("receiverRaw-dict", "InvalidBase64"),
                              ("receiver-phase-list", "InvalidDiscriminator")):
            with self.subTest(problem=problem):
                changed = copy.deepcopy(wrapper)
                if problem == "phase-list":
                    changed["phase"] = [marker]
                elif problem == "receiverRaw-dict":
                    changed["receiverRaw"] = {"secret": marker}
                else:
                    changed_child = copy.deepcopy(child)
                    changed_child["phase"] = [marker]
                    raw = lan.canonical(changed_child)
                    changed["receiverRaw"], changed["receiverRawSha256"] = base64.b64encode(raw).decode(), lan.sha(raw)
                inputs = {paths["plan"]: lan.canonical(fixture["plan"]), paths["records"]: lan.canonical(changed) + b"\n",
                          paths["witnesses"]: b'[]', paths["cases"]: b'[]'}
                output, error = io.BytesIO(), io.StringIO()
                arguments = ["collect"] + [argument for key, value in paths.items() for argument in ("--" + key, value)]
                with mock.patch.object(lan, "read_file", side_effect=lambda path, *args: inputs[path]), \
                     mock.patch.object(lan, "create_file") as create, \
                     mock.patch.object(lan.sys, "stdout", types.SimpleNamespace(buffer=output)), \
                     mock.patch.object(lan.sys, "stderr", error):
                    self.assertEqual(lan.main(arguments), 2)
                result = lan.decode(output.getvalue())
                self.assertEqual(result["verdict"], "Invalid")
                self.assertEqual(result["reasons"], [code])
                self.assertEqual(error.getvalue(), "")
                for private in (marker, "private-operator-path", "Traceback", "lan.py"):
                    self.assertNotIn(private, output.getvalue().decode())
                create.assert_not_called()

    def test_collect_does_not_hide_unrelated_program_type_error(self):
        output = io.BytesIO()
        with mock.patch.object(lan, "read_file", side_effect=[lan.canonical(example_plan()), b'', b'[]', b'[]']), \
             mock.patch.object(lan, "bundle_value", side_effect=TypeError("deliberate program defect")), \
             mock.patch.object(lan, "create_file") as create, \
             mock.patch.object(lan.sys, "stdout", types.SimpleNamespace(buffer=output)):
            with self.assertRaises(TypeError):
                lan.main(["collect", "--plan", "plan", "--records", "records", "--witnesses", "witnesses",
                          "--cases", "cases", "--output", "output"])
        self.assertEqual(output.getvalue(), b'')
        create.assert_not_called()

    def test_source_observation_and_prepare_are_not_execution_or_secret_output(self):
        with tempfile.TemporaryDirectory() as directory:
            p = example_plan()
            root = Path(directory)
            # The real checkout is group-writable and correctly cannot become a private
            # execution source merely because a test wants it. Use a new safe fixture copy.
            for _, path in lan.SOURCE_ITEMS:
                lan.create_file(root / path, (ROOT / path).read_bytes())
            with mock.patch.object(lan, "__file__", str(root / "lan.py")):
                p["binding"]["receiverSourceSha256"] = lan.execution_sources()[2]
                lan.create_file(root / "input", lan.canonical(p))
                output = io.BytesIO()
                with mock.patch.object(lan.sys, "stdout", types.SimpleNamespace(buffer=output)):
                    result = lan.main(["prepare", "--input", str(root / "input"),
                                       "--output-directory", str(root / "prepared")])
            value = lan.decode(output.getvalue())
            self.assertEqual(result, 0)
            self.assertEqual(value["phase"], "PreparedOnly")
            self.assertEqual(value["schema"], "polaris-pc-lan-prepared-v1")
            for secret in (str(root), p["echoNonce"], "receiverInstanceId", "socketInstanceId"):
                self.assertNotIn(secret, output.getvalue().decode())
            self.assertFalse(value["outboundReady"])
            self.assertEqual(lan.read_file(root / "prepared" / "plan.json"), lan.canonical(p))

    def test_serve_pending_before_file_access_spawn_or_any_socket(self):
        output = io.BytesIO()
        with mock.patch.object(lan.sys, "stdout", types.SimpleNamespace(buffer=output)), \
             mock.patch.object(lan, "read_file", side_effect=AssertionError("file accessed")), \
             mock.patch.object(lan, "OwnedPopen", side_effect=AssertionError("spawned")), \
             mock.patch.object(lan.socket, "socket", side_effect=AssertionError("socket created")):
            self.assertEqual(lan.main(["serve", "--plan", "/secret-path", "--execution-input", "/private-input"]), 2)
        value = lan.decode(output.getvalue())
        self.assertEqual(value["reasons"], ["ExecutionNotFrozen"])
        self.assertNotIn("/secret-path", output.getvalue().decode())

    def test_cli_errors_do_not_reproduce_arbitrary_arguments(self):
        output = io.StringIO()
        with mock.patch.object(lan.sys, "stderr", output), self.assertRaises(SystemExit):
            lan.main(["--unexpected-secret-value=/private-token"])
        self.assertEqual(output.getvalue(), "InvalidCLI\n")


if __name__ == "__main__":
    unittest.main(verbosity=2)
