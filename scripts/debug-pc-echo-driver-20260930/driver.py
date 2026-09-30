#!/usr/bin/env python3
"""Private root adapter for the existing PC controller and Android original batch.

Importing this module performs no child, network, kernel or device operation. The explicit
run entry retains original source/Ready/installed-package bytes, starts the existing exact
controller on the main thread, and services one finite app-private authenticated batch.
No JSON approval bit, environment hash or cached Ready can issue CurrentPcReady.
"""
import argparse
import base64
import hashlib
import hmac
import json
import os
from pathlib import Path
import re
import select
import subprocess
import threading
import time
import types

MAXIMUM = 65536
PACKAGE = "com.polaris2.app.debug"
PRIVATE_DIRECTORY = "files/debug-pc-echo-v1"
DOMAIN = b"polaris-debug-pc-echo-private-v1\0"
REQUEST_KEYS = {"schema", "requestId", "action", "phase", "sequence", "bootNonce", "sessionId",
                "nonce", "planSha256", "apkSha256", "expectedSourcePin"}
PHASES = [p + "-" + k + "-" + position for p in ("tcp", "udp")
          for k in ("wrong-nonce", "foreign-peer", "wrong-port-admission")
          for position in ("before", "negative", "after", "final")]


class Unavailable(Exception):
    def __str__(self):
        return "Private coordinated batch unavailable"


def require(condition):
    if not condition:
        raise Unavailable()


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


def canonical(value):
    raw = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False,
                     allow_nan=False).encode("utf-8")
    require(len(raw) <= MAXIMUM)
    return raw


def decode(raw):
    require(type(raw) is bytes and 2 <= len(raw) <= MAXIMUM)
    def pairs(items):
        value = {}
        for key, item in items:
            require(key not in value)
            value[key] = item
        return value
    try:
        value = json.loads(raw.decode("utf-8", "strict"), object_pairs_hook=pairs,
                           parse_float=lambda _: (_ for _ in ()).throw(Unavailable()),
                           parse_constant=lambda _: (_ for _ in ()).throw(Unavailable()))
    except (ValueError, UnicodeError, RecursionError):
        raise Unavailable() from None
    def bounded(item, depth=0):
        require(depth <= 8)
        if type(item) is dict:
            require(len(item) <= 64)
            for k, v in item.items():
                require(type(k) is str and len(k) <= MAXIMUM and not any(0xd800 <= ord(c) <= 0xdfff for c in k))
                bounded(v, depth + 1)
        elif type(item) is list:
            require(len(item) <= 128)
            for v in item:
                bounded(v, depth + 1)
        elif type(item) is str:
            require(len(item) <= MAXIMUM and not any(0xd800 <= ord(c) <= 0xdfff for c in item))
        else:
            require(item is None or type(item) is bool or type(item) is int and -(1 << 63) <= item < 1 << 63)
    bounded(value)
    require(type(value) is dict)
    return value


def envelope(secret, value):
    body = canonical(value)
    return canonical({"body": base64.b64encode(body).decode("ascii"),
                      "mac": hmac.new(secret, DOMAIN + body, hashlib.sha256).hexdigest()})


def authenticate(secret, raw):
    value = decode(raw)
    require(set(value) == {"body", "mac"} and type(value["body"]) is str and
            type(value["mac"]) is str and re.fullmatch("[0-9a-f]{64}", value["mac"]))
    try:
        body = base64.b64decode(value["body"], validate=True)
    except ValueError:
        raise Unavailable() from None
    require(hmac.compare_digest(hmac.new(secret, DOMAIN + body, hashlib.sha256).hexdigest(), value["mac"]))
    result = decode(body)
    require(canonical(result) == body)
    return result


def remaining(original_ms, anchor_ns, received_ns):
    require(type(original_ms) is int and 1 <= original_ms <= 120000 and
            type(anchor_ns) is int and type(received_ns) is int and 0 <= anchor_ns <= received_ns)
    # Ceil the full request roundtrip. Remote numeric monotonic clocks are never compared.
    value = original_ms - (received_ns - anchor_ns + 999999) // 1000000
    require(value > 0)
    return value


class OriginalRecords:
    """Bounded retained stream from our controller's sole original output pipe."""
    def __init__(self, lan, raw_plan, sources, manifest):
        self.lan, self.raw_plan, self.sources, self.manifest = lan, raw_plan, sources, manifest
        self.condition = threading.Condition()
        self.records, self.receiver = [], []
        self.ready_raw, self.closed, self.controller_id = None, False, None
        self.event_sequence = 0

    def receive(self, raw):
        item = self.lan.controller_record(raw)
        require(item["sourceKind"] == "OperatorDeclared")
        with self.condition:
            require(len(self.records) < 128 and item["eventSeq"] == self.event_sequence + 1)
            if self.controller_id is None:
                self.controller_id = item["controllerInstanceId"]
            require(self.controller_id == item["controllerInstanceId"])
            self.event_sequence += 1
            self.records.append(raw)
            if item["phase"] == "ReceiverRecord":
                original = self.lan.raw_base64(item["receiverRaw"])
                child = self.lan.receiver_record(original)
                require(child["binding"] == self.lan.plan(self.lan.decode(self.raw_plan))["binding"] and
                        child["pcPlanSha256"] == self.lan.plan_sha(self.raw_plan))
                require(child["receiverSeq"] == len(self.receiver) + 1)
                self.receiver.append((original, child))
                if child["phase"] == "Ready":
                    require(self.ready_raw is None and len(self.receiver) == 1)
                    self.ready_raw = original
                if child["phase"] in ("Closed", "Unknown"):
                    self.closed = True
            elif item["phase"] in ("Unknown", "ChildWait", "StopRequested"):
                self.closed = True
            self.condition.notify_all()

    def wait(self, predicate, deadline_ns):
        with self.condition:
            while True:
                require(not self.closed)
                found = predicate(self.receiver)
                if found is not None:
                    return found
                seconds = (deadline_ns - time.monotonic_ns()) / 1e9
                require(seconds > 0)
                self.condition.wait(min(.1, seconds))

    def handoff(self, request_id, control_fd):
        ready_raw = self.wait(lambda _: self.ready_raw, time.monotonic_ns() + 2000000000)
        ready = self.lan.receiver_record(ready_raw)
        anchor = time.monotonic_ns()  # before the actual original controller request write
        raw = canonical({"command": "readyHandoff", "requestId": request_id}) + b"\n"
        require(os.write(control_fd, raw) == len(raw))
        original, item = self.wait(lambda records: next(((r, v) for r, v in records
            if v["phase"] == "ReadyHandoff" and v["requestId"] == request_id), None), anchor + 2000000000)
        self.lan.handoff_check(ready, sha(ready_raw), item, request_id)
        budget = remaining(item["remainingLifetimeMs"], anchor, time.monotonic_ns())
        target = self.lan.target_from_original_ready(self.raw_plan, ready_raw)
        return {"schema": "polaris-debug-pc-echo-response-v1", "action": "ready", "requestId": request_id,
                "target": target, "remainingLifetimeMs": budget, "remainingRequests": item["remainingRequests"],
                "originalReady": base64.b64encode(ready_raw).decode("ascii"),
                "originalHandoff": base64.b64encode(original).decode("ascii")}

    def snapshot(self, request_id, control_fd):
        with self.condition:
            require(not self.closed and self.ready_raw is not None)
            baseline = len(self.receiver)
        anchor = time.monotonic_ns()
        raw = canonical({"command": "snapshot"}) + b"\n"
        require(os.write(control_fd, raw) == len(raw))
        original, item = self.wait(lambda records: next(((r, v) for r, v in records[baseline:]
            if v["phase"] == "Counters"), None), anchor + 2000000000)
        ready = self.lan.receiver_record(self.ready_raw)
        require(not item["sealed"] and item["receiverInstanceId"] == ready["receiverInstanceId"] and
                all(item[k] == ready[k] for k in ("binding", "pcPlanSha256", "controllerInstanceId")) and
                "observedMonotonicNs" in item and ready["startedMonotonicNs"] <= item["observedMonotonicNs"] < ready["expiresMonotonicNs"])
        return {"schema": "polaris-debug-pc-echo-response-v1", "action": "snapshot", "requestId": request_id,
                "originalCounters": base64.b64encode(original).decode("ascii")}


class AdbPrivateChannel:
    """Fixed USB/run-as verbs; no root, listener, arbitrary package/path/shell input."""
    def __init__(self, serial):
        require(type(serial) is str and re.fullmatch(r"[A-Za-z0-9_.-]{1,128}", serial))
        self.prefix = ["adb", "-s", serial]

    def call(self, args, data=None, maximum=MAXIMUM, timeout=2):
        # Future effect consumer only. No source/pure test calls this method.
        with subprocess.Popen(self.prefix + args, stdin=subprocess.PIPE if data is not None else subprocess.DEVNULL,
                              stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, close_fds=True) as process:
            try:
                out, _ = process.communicate(data, timeout=timeout)
            except subprocess.TimeoutExpired:
                process.kill(); process.communicate(); raise Unavailable() from None
            require(process.returncode == 0 and len(out) <= maximum)
            return out

    def installed_original(self, external_apk):
        transport = self.call(["get-devpath"], maximum=1024)
        require(re.fullmatch(rb"usb:[^\s]{1,512}\r?\n?", transport))
        source = external_apk.read_bytes()
        require(0 < len(source) <= 200 * 1024 * 1024)
        listing = self.call(["shell", "pm", "path", PACKAGE], maximum=1024)
        try:
            line = listing.decode("ascii").strip()
        except UnicodeError:
            raise Unavailable() from None
        require(re.fullmatch(r"package:/data/app/[A-Za-z0-9_./=~+-]+/base\.apk", line) and ".." not in line)
        actual = self.call(["exec-out", "cat", line[len("package:"):]], maximum=200 * 1024 * 1024, timeout=15)
        require(hmac.compare_digest(hashlib.sha256(actual).digest(), hashlib.sha256(source).digest()))
        # Keep original bytes and actual package manager result in the root-owned run evidence.
        return {"package": PACKAGE, "usbDevpathRaw": transport, "pmPathRaw": listing, "installedBytes": actual,
                "externalArtifactBytes": source, "sha256": sha(actual)}

    def request(self):
        try:
            return self.call(["exec-out", "run-as", PACKAGE, "cat", PRIVATE_DIRECTORY + "/request.json"])
        except Unavailable:
            return None

    def reply(self, raw):
        require(type(raw) is bytes and 2 <= len(raw) <= MAXIMUM)
        # Fixed command text, no interpolation of operator/request fields into a shell.
        script = "umask 077; cat > files/debug-pc-echo-v1/reply.tmp && mv files/debug-pc-echo-v1/reply.tmp files/debug-pc-echo-v1/reply.json"
        self.call(["shell", "run-as", PACKAGE, "sh", "-c", "'" + script + "'"], raw, maximum=0)


def request_check(value, expected_sequence, first, source_receipt_sha, package_sha):
    require(set(value) == REQUEST_KEYS and value["schema"] == "polaris-debug-pc-echo-request-v1" and
            type(value["sequence"]) is int and value["sequence"] == expected_sequence and 1 <= expected_sequence <= 25)
    for key, size in (("requestId", 32), ("bootNonce", 32), ("sessionId", 32), ("nonce", 48), ("planSha256", 64)):
        require(type(value[key]) is str and re.fullmatch("[0-9a-f]{%d}" % size, value[key]))
    require(value["apkSha256"] == package_sha and value["expectedSourcePin"] == source_receipt_sha)
    require(value["action"] == ("ready" if expected_sequence == 1 else "snapshot") and
            value["phase"] == ("ready" if expected_sequence == 1 else PHASES[expected_sequence - 2]))
    if first is not None:
        require(all(value[k] == first[k] for k in ("bootNonce", "sessionId", "nonce", "planSha256", "apkSha256", "expectedSourcePin")))
    return value


def package_binding(source_receipt, binding, installed):
    # This is the external build producer's original source+artifact receipt, retained
    # independently of the app. Neither the app plan nor a same-named config proves it.
    require(source_receipt.get("commit") == binding["androidCandidateSha"])
    artifact = source_receipt.get("packageArtifact")
    require(type(artifact) is dict and set(artifact) == {"sha256", "bytes", "packageName", "variant"})
    require(artifact["packageName"] == PACKAGE and artifact["variant"] == "Debug" and
            type(artifact["bytes"]) is int and artifact["bytes"] == len(installed["installedBytes"]) and
            artifact["sha256"] == installed["sha256"] == binding["androidPackageSha256"] and
            installed["package"] == PACKAGE and sha(installed["externalArtifactBytes"]) == installed["sha256"])


def load_original_sources(directory, receipt_raw):
    receipt = decode(receipt_raw)
    require(type(receipt.get("executionSourceManifest")) is dict)
    sources = {name: (directory / name).read_bytes() for name in ("lan.py", "private-windows.ps1")}
    for name, raw in sources.items():
        require(type(raw) is bytes and 0 < len(raw) <= 262144)
    require(receipt["executionSourceManifest"] == {"schema": "polaris-pc-lan-execution-source-v1", "items": [
        {"role": role, "path": name, "sha256": sha(sources[name])}
        for role, name in (("controller-receiver", "lan.py"), ("windows-private-file", "private-windows.ps1"))]})
    # Execute precisely the retained controller-receiver source, not a later filesystem import.
    module = types.ModuleType("polaris_original_pc_lan")
    module.__file__ = str(directory / "lan.py")
    exec(compile(sources["lan.py"], module.__file__, "exec"), module.__dict__)
    manifest, source_hash = module.source_manifest(sources)
    require(manifest == receipt["executionSourceManifest"])
    return module, sources, manifest, source_hash


def run(args):
    require(os.environ.get("POLARIS_NO_KERNEL_RUN") != "1" and os.name == "posix")
    root = Path(__file__).resolve().parent.parent
    pc_receipt_raw = Path(args.pc_source_receipt).read_bytes()
    lan, sources, manifest, source_hash = load_original_sources(root / "pc-lan-debug-20260930", pc_receipt_raw)
    raw_plan = lan.read_file(Path(args.pc_plan), MAXIMUM, True)
    plan = lan.plan(lan.decode(raw_plan)); require(plan["sourceKind"] == "OperatorDeclared")
    source_receipt = Path(args.android_source_receipt).read_bytes()
    source_value = decode(source_receipt)
    require(source_hash == plan["binding"]["receiverSourceSha256"])
    secret_hex = lan.read_file(Path(args.session_secret), 64, True)
    require(re.fullmatch(b"[0-9a-f]{64}", secret_hex))
    secret = bytearray.fromhex(secret_hex.decode("ascii"))
    channel = AdbPrivateChannel(args.serial)
    installed = channel.installed_original(Path(args.apk))
    package_binding(source_value, plan["binding"], installed)
    records = OriginalRecords(lan, raw_plan, sources, manifest)
    control_read, control_write = os.pipe(); record_read, record_write = os.pipe()
    stop = threading.Event(); failures = []
    evidence = Path(args.evidence); evidence.mkdir(mode=0o700, parents=False, exist_ok=False)
    os.chmod(evidence, 0o700)
    # Original bytes are never overwritten/reconstructed, and nothing private is stdout/logged.
    for name, raw in (("plan.json", raw_plan), ("pc-source-receipt.json", pc_receipt_raw),
                      ("android-source-receipt.json", source_receipt), ("usb-devpath.raw", installed["usbDevpathRaw"]),
                      ("installed-package-path.raw", installed["pmPathRaw"]),
                      ("installed-package.apk", installed["installedBytes"]), ("external-artifact.apk", installed["externalArtifactBytes"])):
        fd = os.open(evidence / name, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "wb") as out:
            out.write(raw)
    for name, raw in sources.items():
        (evidence / name).write_bytes(raw); os.chmod(evidence / name, 0o600)

    def output_worker():
        pending = bytearray()
        try:
            with (evidence / "original-controller.ndjson").open("xb") as out:
                os.chmod(out.name, 0o600)
                while True:  # consume the original terminal tail through actual pipe EOF
                    if not select.select([record_read], [], [], .1)[0]:
                        continue
                    block = os.read(record_read, 16385)
                    if not block:
                        break
                    pending.extend(block); require(len(pending) <= 32768)
                    while b"\n" in pending:
                        raw, _, tail = pending.partition(b"\n"); pending = bytearray(tail)
                        records.receive(bytes(raw)); out.write(raw + b"\n"); out.flush()
                require(not pending)
        except BaseException as error:
            failures.append(error); stop.set()

    def private_worker():
        first, last_id, sequence = None, None, 1
        try:
            while not stop.is_set() and sequence <= 25:
                raw = channel.request()
                if raw is None:
                    time.sleep(.02); continue
                value = authenticate(secret, raw)
                if value["requestId"] == last_id:
                    time.sleep(.02); continue
                request_check(value, sequence, first, sha(source_receipt), installed["sha256"])
                if first is None:
                    first = value
                response = records.handoff(value["requestId"], control_write) if sequence == 1 else records.snapshot(value["requestId"], control_write)
                # The original remaining lease is further reduced while encoding/writing; the
                # app deducts its entire own pre-request roundtrip, which also covers that tail.
                require(channel.request() == raw)  # cancellation/late IPC cannot write a retired request
                channel.reply(envelope(secret, response))
                last_id = value["requestId"]; sequence += 1
            if sequence == 26:
                os.write(control_write, b'{"command":"stop"}\n')
        except BaseException as error:
            failures.append(error); stop.set()
            try:
                os.write(control_write, b'{"command":"stop"}\n')
            except OSError:
                pass

    reader = threading.Thread(target=output_worker, name="polaris-original-records")
    private = threading.Thread(target=private_worker, name="polaris-private-handoff")
    reader.start(); private.start()
    try:
        # This call owns the original Child, unique wait and receiver socket responsibility.
        # There is no replacement spawn/waiter, PID signaling or synthetic Ready path here.
        lan.controller_run(raw_plan, sources, manifest, lan.OutputQueue(record_write), control_read)
    finally:
        stop.set(); os.close(record_write)
        reader.join(3); private.join(3)
        require(not reader.is_alive() and not private.is_alive())
        for fd in (record_read, control_read, control_write):
            os.close(fd)
        secret[:] = bytes(len(secret))
    require(not failures)


def main(argv=None):
    parser = argparse.ArgumentParser(description="Existing coordinated batch private root adapter")
    for name in ("serial", "pc-plan", "pc-source-receipt", "android-source-receipt", "session-secret", "apk", "evidence"):
        parser.add_argument("--" + name, required=True)
    args = parser.parse_args(argv)
    try:
        run(args)
    except (Unavailable, OSError, ValueError):
        print("Private coordinated batch unavailable")
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
