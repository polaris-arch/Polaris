"""Owned loopback fixtures, not a proxy to the Internet or a replacement core.

Each SOCKS destination is observed but never dialled. Admission changes affect
new connections only: an old echo stream is deliberately left alive. DNS answers
have TTL 0 and are served over both UDP and TCP. No application defaults are
written here; callers must observe an empty-HOME dashboard before using a patch.
"""
from __future__ import annotations

import copy
import hashlib
import ipaddress
import socket
import struct
import threading

DOMAIN = "polaris-package-acceptance.invalid"
IO_TIMEOUT = 3.0
MAX_CONNECTIONS = 128


def require(ok, message):
    if not ok:
        raise ValueError(message)


def recv_exact(sock, count):
    data = bytearray()
    while len(data) < count:
        block = sock.recv(count - len(data))
        if not block:
            raise EOFError("fixture peer closed")
        data.extend(block)
    return bytes(data)


def loopback(host):
    try:
        return ipaddress.ip_address(host).is_loopback
    except ValueError:
        return False


class OwnedTCP:
    def __init__(self):
        self.listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        try:
            self.listener.bind(("127.0.0.1", 0))
            self.listener.listen(32)
            self.listener.settimeout(0.1)
        except BaseException:
            self.listener.close()
            raise
        self.address = self.listener.getsockname()
        self.stop = threading.Event()
        self.lock = threading.Lock()
        self.sockets = set()
        self.workers = []
        self.failures = []
        self.total_connections = 0
        self.thread = threading.Thread(target=self._accept, daemon=False)
        self.thread.start()

    def _accept(self):
        while not self.stop.is_set():
            try:
                conn, peer = self.listener.accept()
            except socket.timeout:
                continue
            except OSError:
                if not self.stop.is_set():
                    self.failures.append("listener-failure")
                return
            with self.lock:
                self.total_connections += 1
                allowed = loopback(peer[0]) and len(self.sockets) < MAX_CONNECTIONS
                allowed = allowed and self.total_connections <= 2048
                if allowed:
                    self.sockets.add(conn)
            if not allowed:
                conn.close()
                self.failures.append("connection-bound-or-peer")
                continue
            conn.settimeout(IO_TIMEOUT)
            worker = threading.Thread(target=self._serve, args=(conn,), daemon=False)
            self.workers.append(worker)
            worker.start()

    def _serve(self, conn):
        try:
            self.handle(conn)
        except (EOFError, ConnectionError, socket.timeout):
            # Expected when a measured client cancels its own connection.
            pass
        except OSError:
            if not self.stop.is_set():
                self.failures.append("socket-failure")
        except (ValueError, UnicodeError):
            self.failures.append("invalid-fixture-protocol")
        finally:
            with self.lock:
                self.sockets.discard(conn)
            conn.close()

    def close(self):
        self.stop.set()
        self.listener.close()
        with self.lock:
            sockets = list(self.sockets)
        for conn in sockets:
            try:
                conn.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
        self.thread.join(IO_TIMEOUT + 1)
        for worker in self.workers:
            worker.join(IO_TIMEOUT + 1)
        require(not self.thread.is_alive() and not any(w.is_alive() for w in self.workers),
                "fixture cleanup unknown")


class HealthServer(OwnedTCP):
    def handle(self, conn):
        data = conn.recv(4096)
        if not data:
            return
        require(data.startswith((b"GET /health ", b"HEAD /health ")), "health path differs")
        conn.sendall(b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")


class SocksBackend(OwnedTCP):
    def __init__(self, label, domain=DOMAIN, latency=0.0):
        require(label in ("A", "B"), "fixture label differs")
        self.label, self.domain = label, domain
        self.admission, self.latency = True, latency
        self.targets = []
        super().__init__()

    def set_admission(self, enabled):
        require(isinstance(enabled, bool), "explicit admission required")
        with self.lock:
            self.admission = enabled

    def set_latency(self, seconds):
        require(isinstance(seconds, (int, float)) and 0 <= seconds <= 2, "latency bound")
        with self.lock:
            self.latency = seconds

    def handle(self, conn):
        version, count = recv_exact(conn, 2)
        require(version == 5 and count > 0, "SOCKS greeting differs")
        require(0 in recv_exact(conn, count), "fixture requires no-auth SOCKS")
        conn.sendall(b"\x05\x00")
        version, command, reserved, kind = recv_exact(conn, 4)
        require((version, command, reserved) == (5, 1, 0), "fixture only supports CONNECT")
        if kind == 1:
            host = socket.inet_ntop(socket.AF_INET, recv_exact(conn, 4))
        elif kind == 4:
            host = socket.inet_ntop(socket.AF_INET6, recv_exact(conn, 16))
        elif kind == 3:
            length = recv_exact(conn, 1)[0]
            require(length > 0, "empty SOCKS domain")
            host = recv_exact(conn, length).decode("ascii")
        else:
            raise ValueError("SOCKS address differs")
        port = struct.unpack("!H", recv_exact(conn, 2))[0]
        with self.lock:
            admitted = self.admission and (loopback(host) or host == self.domain)
            row = {"connection": len(self.targets) + 1, "host": host, "port": port,
                   "admitted": admitted, "bytesIn": 0, "bytesOut": 0, "chunks": [], "kind": None}
            self.targets.append(row)
            latency = self.latency
        if not admitted:
            conn.sendall(b"\x05\x02\x00\x01\x7f\x00\x00\x01\x00\x00")
            return
        if latency:
            self.stop.wait(latency)
        conn.sendall(b"\x05\x00\x00\x01\x7f\x00\x00\x01\x00\x00")
        conn.settimeout(0.1)
        while not self.stop.is_set():
            try:
                data = conn.recv(4096)
            except socket.timeout:
                continue
            if not data:
                break
            if row["kind"] is None:
                row["kind"] = "health" if data.startswith((b"GET ", b"HEAD ")) else "echo"
            if row["kind"] == "health":
                reply = b"HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            else:
                reply = b"".join(self.label.encode() + bytes([byte]) for byte in data)
            with self.lock:
                row["bytesIn"] += len(data)
                row["bytesOut"] += len(reply)
                row["chunks"].append({"bytes": len(data), "sha256": hashlib.sha256(data).hexdigest()})
            conn.sendall(reply)
            if row["kind"] == "health":
                break

    def snapshot(self):
        with self.lock:
            return {"label": self.label, "address": list(self.address), "admission": self.admission,
                    "targets": copy.deepcopy(self.targets), "failures": list(self.failures)}


def dns_question(packet):
    require(len(packet) >= 12 and packet[2] & 0x80 == 0 and packet[4:6] == b"\x00\x01",
            "one DNS query required")
    offset, labels = 12, []
    while True:
        require(offset < len(packet), "DNS label truncated")
        length = packet[offset]
        offset += 1
        if not length:
            break
        require(length <= 63 and offset + length <= len(packet), "DNS label differs")
        labels.append(packet[offset:offset + length].decode("ascii"))
        offset += length
        require(offset <= 267, "DNS name bound")
    require(offset + 4 <= len(packet), "DNS question truncated")
    kind, cls = struct.unpack("!HH", packet[offset:offset + 4])
    require(cls == 1, "DNS class differs")
    return ".".join(labels).lower(), kind, offset + 4


class Resolver(OwnedTCP):
    def __init__(self, answer, domain=DOMAIN):
        require(loopback(answer), "DNS answer must be loopback")
        self.answer, self.domain, self.queries = answer, domain, []
        super().__init__()
        self.udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        try:
            self.udp.bind(self.address)
            self.udp.settimeout(0.1)
            self.udp_thread = threading.Thread(target=self._udp, daemon=False)
            self.udp_thread.start()
        except BaseException:
            self.udp.close()
            super().close()
            raise

    def response(self, packet, transport):
        name, kind, end = dns_question(packet)
        known = name == self.domain
        answers = int(known and kind == 1)
        with self.lock:
            self.queries.append({"name": name, "type": kind, "transport": transport,
                                 "answer": self.answer if answers else None, "ttl": 0,
                                 "querySha256": hashlib.sha256(packet).hexdigest()})
        flags = 0x8180 if known else 0x8183
        response = packet[:2] + struct.pack("!HHHHH", flags, 1, answers, 0, 0) + packet[12:end]
        if answers:
            response += b"\xc0\x0c\x00\x01\x00\x01" + struct.pack("!IH", 0, 4)
            response += socket.inet_aton(self.answer)
        return response

    def _udp(self):
        while not self.stop.is_set():
            try:
                data, peer = self.udp.recvfrom(4096)
                require(loopback(peer[0]), "DNS peer differs")
                self.udp.sendto(self.response(data, "udp"), peer)
            except socket.timeout:
                pass
            except (ValueError, UnicodeError):
                self.failures.append("invalid-dns-query")
            except OSError:
                if not self.stop.is_set():
                    self.failures.append("dns-socket-failure")
                return

    def handle(self, conn):
        size = struct.unpack("!H", recv_exact(conn, 2))[0]
        require(12 <= size <= 4096, "DNS TCP bound")
        response = self.response(recv_exact(conn, size), "tcp")
        conn.sendall(struct.pack("!H", len(response)) + response)

    def snapshot(self):
        with self.lock:
            return {"address": list(self.address), "queries": copy.deepcopy(self.queries),
                    "failures": list(self.failures)}

    def close(self):
        super().close()
        self.udp.close()
        self.udp_thread.join(IO_TIMEOUT + 1)
        require(not self.udp_thread.is_alive(), "DNS cleanup unknown")


def socks_connect(address, host=DOMAIN, port=80):
    require(loopback(address[0]) and 0 < address[1] < 65536, "loopback proxy required")
    name = host.encode("ascii")
    require(0 < len(name) < 256 and 0 < port < 65536, "SOCKS destination bound")
    conn = socket.create_connection(address, timeout=IO_TIMEOUT)
    try:
        conn.sendall(b"\x05\x01\x00")
        require(recv_exact(conn, 2) == b"\x05\x00", "SOCKS method rejected")
        conn.sendall(b"\x05\x01\x00\x03" + bytes([len(name)]) + name + struct.pack("!H", port))
        head = recv_exact(conn, 4)
        require(head[:3] == b"\x05\x00\x00", "SOCKS CONNECT rejected")
        if head[3] == 1:
            recv_exact(conn, 4)
        elif head[3] == 4:
            recv_exact(conn, 16)
        elif head[3] == 3:
            recv_exact(conn, recv_exact(conn, 1)[0])
        else:
            raise ValueError("SOCKS reply address differs")
        recv_exact(conn, 2)
        return conn
    except BaseException:
        conn.close()
        raise


def exchange(conn, label, nonce):
    require(label in ("A", "B"), "expected backend differs")
    payload = nonce.encode("ascii") if isinstance(nonce, str) else bytes(nonce)
    require(0 < len(payload) <= 1024, "nonce bound")
    conn.sendall(payload)
    expected = b"".join(label.encode() + bytes([byte]) for byte in payload)
    require(recv_exact(conn, len(expected)) == expected, "live backend echo differs")
    return {"backend": label, "bytes": len(payload), "nonceSha256": hashlib.sha256(payload).hexdigest()}


class Network:
    def __init__(self, domain=DOMAIN):
        require(domain == DOMAIN, "reviewed fixture domain required")
        self.domain = domain
        self._owned = []
        try:
            for name, constructor in [
                ("health", HealthServer), ("a", lambda: SocksBackend("A", domain)),
                ("b", lambda: SocksBackend("B", domain, 0.15)),
                ("dns_a", lambda: Resolver("127.0.0.2", domain)),
                ("dns_b", lambda: Resolver("127.0.0.3", domain)),
            ]:
                fixture = constructor()
                self._owned.append(fixture)
                setattr(self, name, fixture)
        except BaseException:
            cleanup_errors = []
            for fixture in reversed(self._owned):
                try:
                    fixture.close()
                except Exception as error:
                    cleanup_errors.append(type(error).__name__)
            require(not cleanup_errors, "partial fixture cleanup unknown")
            raise
        self.health_url = f"http://127.0.0.1:{self.health.address[1]}/health"
        self.closed = False

    def config_patch(self, mixed_port, control_port, secret):
        require(isinstance(secret, str) and len(secret) >= 16, "private test secret required")
        require(0 < mixed_port < 65536 and 0 < control_port < 65536 and mixed_port != control_port,
                "distinct loopback listener ports required")
        def resource(name, port):
            return {"id": name, "name": name, "enabled": True, "type": "udp",
                    "endpoint": {"host": "127.0.0.1", "port": port}, "outbound": {"type": "direct"}}
        route = {"id": "loop-route", "type": "domain", "values": [self.domain], "action": "proxy",
                 "enabled": True, "targetServerId": "node-a",
                 "effects": {"route": {"enabled": True, "action": "proxy", "targetServerId": "node-a"}}}
        patch = {"proxyModeType": "manual", "proxyMode": "smart", "mixedPort": mixed_port,
                 "controlPort": control_port, "clashApiSecret": secret, "selectedServerId": "node-a",
                 "speedTestUrl": self.health_url,
                 "servers": [{"id": "node-" + label.lower(), "name": "Loop " + label,
                              "protocol": "socks", "address": "127.0.0.1", "port": backend.address[1]}
                             for label, backend in [("A", self.a), ("B", self.b)]],
                 "trafficRules": [copy.deepcopy(route)], "policyRules": [copy.deepcopy(route)],
                 "customRules": [copy.deepcopy(route)], "routeRuleOrder": ["loop-route"],
                 "dnsRules": [{"id": "loop-dns", "type": "domain", "values": [self.domain],
                               "action": "proxy", "enabled": True,
                               "effects": {"dns": {"enabled": True, "action": {"type": "server", "serverId": "loop-a"}}}}],
                 "dnsRuleOrder": ["loop-dns"],
                 "dnsServers": [resource(name, self.dns_a.address[1]) for name in
                                ["builtin-domestic", "builtin-remote", "builtin-bootstrap", "loop-a"]]
                               + [resource("loop-b", self.dns_b.address[1])],
                 "dnsDefaults": {"directServerId": "builtin-domestic", "proxyServerId": "builtin-remote",
                                 "unmatchedAction": {"type": "reject", "method": "drop"},
                                 "realQueryFallbackAction": {"type": "reject", "method": "drop"},
                                 "connectionResolution": "dnsRules"},
                 "dnsConfig": {"domesticDns": f"udp://127.0.0.1:{self.dns_a.address[1]}",
                               "foreignDns": f"udp://127.0.0.1:{self.dns_a.address[1]}",
                               "enableFakeIp": False, "fakeIpToggleMigrated": True, "fakeIpTunAutoEnable": False,
                               "optimisticCache": False, "takeoverSystemDns": False, "nodeResolverMigrated": True,
                               "resolveNodeDomainsAhead": False, "nodeResolverSingle": "loop-node",
                               "nodeResolverPool": ["loop-node"],
                               "nodeResolverCustom": [{"id": "loop-node", "spec": f"udp://127.0.0.1:{self.dns_a.address[1]}"}]}}
        for key in ["allowLan", "bypassLAN", "appRoutingEnabled", "autoSelectEnabled", "autoSwitchNode",
                    "autoCheckUpdate", "autoUpdateSubscriptionOnStart", "ruleResourceAutoUpdate",
                    "interruptConnectionsOnSwitch", "enableIPv6", "blockQuic"]:
            patch[key] = False
        for key in ["ruleResources", "customRuleSets", "appRules", "networkProfiles", "subscriptions",
                    "autoSelectPins", "dnsServerGroups"]:
            patch[key] = []
        return patch

    @staticmethod
    def changed_graph(config):
        result = copy.deepcopy(config)
        for key in ["trafficRules", "policyRules", "customRules"]:
            result[key][0]["targetServerId"] = "node-b"
            result[key][0]["effects"]["route"]["targetServerId"] = "node-b"
        result["dnsRules"][0]["effects"]["dns"]["action"]["serverId"] = "loop-b"
        return result

    def snapshot(self):
        return {"domain": self.domain, "a": self.a.snapshot(), "b": self.b.snapshot(),
                "dnsA": self.dns_a.snapshot(), "dnsB": self.dns_b.snapshot(), "answerTtl": 0,
                "healthFailures": list(self.health.failures), "closed": self.closed}

    def close(self):
        if self.closed:
            return
        errors = []
        for fixture in reversed(self._owned):
            try:
                fixture.close()
            except Exception as error:
                errors.append(type(error).__name__)
        self.closed = not errors
        require(not errors, "fixture cleanup unknown")

    def __enter__(self):
        return self

    def __exit__(self, *_):
        self.close()
