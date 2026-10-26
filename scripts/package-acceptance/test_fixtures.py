#!/usr/bin/env python3
"""Local fixture/probe contracts; never launches Polaris or its bundled core."""
import hashlib
import importlib.util
import socket
import re
import struct
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("package_fixtures", ROOT / "fixtures.py")
fixtures = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixtures)


def query(name=fixtures.DOMAIN, kind=1):
    labels = b"".join(bytes([len(label)]) + label.encode() for label in name.split(".")) + b"\0"
    return b"\x12\x34\x01\x00\x00\x01\x00\x00\x00\x00\x00\x00" + labels + struct.pack("!HH", kind, 1)


class FixtureTests(unittest.TestCase):
    def test_admission_does_not_disconnect_existing_stream(self):
        backend = fixtures.SocksBackend("A")
        try:
            with fixtures.socks_connect(backend.address) as old:
                first = fixtures.exchange(old, "A", "pacc-before")
                backend.set_admission(False)
                with self.assertRaisesRegex(ValueError, "CONNECT rejected"):
                    fixtures.socks_connect(backend.address)
                second = fixtures.exchange(old, "A", "pacc-after")
            self.assertEqual(first["nonceSha256"], hashlib.sha256(b"pacc-before").hexdigest())
            self.assertNotEqual(first["nonceSha256"], second["nonceSha256"])
            rows = backend.snapshot()["targets"]
            self.assertEqual([r["admitted"] for r in rows], [True, False])
            self.assertEqual(rows[0]["kind"], "echo")
            self.assertEqual(rows[0]["bytesIn"], len(b"pacc-beforepacc-after"))
        finally:
            backend.close()
        self.assertFalse(backend.thread.is_alive())

    def test_new_connections_have_distinct_backend_evidence(self):
        with fixtures.Network() as network:
            for label, backend in [("A", network.a), ("B", network.b)]:
                with fixtures.socks_connect(backend.address, "127.0.0.2") as conn:
                    self.assertEqual(fixtures.exchange(conn, label, "pacc-unique-" + label)["backend"], label)
            snapshot = network.snapshot()
            self.assertEqual(snapshot["a"]["targets"][0]["host"], "127.0.0.2")
            self.assertEqual(snapshot["b"]["targets"][0]["kind"], "echo")
        self.assertTrue(network.closed)
        network.close()

    def test_nonloopback_targets_rejected_without_forwarding(self):
        backend = fixtures.SocksBackend("A")
        try:
            for target in ["8.8.8.8", "example.org", "169.254.169.254"]:
                with self.assertRaisesRegex(ValueError, "CONNECT rejected"):
                    fixtures.socks_connect(backend.address, target)
            self.assertTrue(all(not row["admitted"] for row in backend.snapshot()["targets"]))
        finally:
            backend.close()

    def test_udp_and_tcp_dns_are_fresh_ttl_zero(self):
        resolver = fixtures.Resolver("127.0.0.3")
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as udp:
                udp.settimeout(2)
                udp.sendto(query(), resolver.address)
                response, _ = udp.recvfrom(4096)
            self.assertEqual(response[:2], b"\x12\x34")
            self.assertEqual(response[-10:], struct.pack("!IH", 0, 4) + socket.inet_aton("127.0.0.3"))
            with socket.create_connection(resolver.address, timeout=2) as tcp:
                packet = query()
                tcp.sendall(struct.pack("!H", len(packet)) + packet)
                length = struct.unpack("!H", fixtures.recv_exact(tcp, 2))[0]
                second = fixtures.recv_exact(tcp, length)
            self.assertEqual(second, response)
            self.assertEqual([row["transport"] for row in resolver.snapshot()["queries"]], ["udp", "tcp"])
            self.assertTrue(all(row["ttl"] == 0 for row in resolver.snapshot()["queries"]))
        finally:
            resolver.close()

    def test_dns_unknown_is_not_resolved_to_external_address(self):
        resolver = fixtures.Resolver("127.0.0.2")
        try:
            response = resolver.response(query("external.invalid"), "unit")
            self.assertEqual(response[3] & 15, 3)
            self.assertEqual(response[6:8], b"\0\0")
            response = resolver.response(query(kind=28), "unit")
            self.assertEqual(response[6:8], b"\0\0")
        finally:
            resolver.close()

    def test_dns_malformed_packets_fail_closed(self):
        for packet in [b"", query()[:13], query()[:12] + b"\xc0\x0c\0\1\0\1", query()[:-2]]:
            with self.subTest(packet=packet), self.assertRaises(ValueError):
                fixtures.dns_question(packet)

    def test_health_uses_loopback_and_real_socks(self):
        with fixtures.Network() as network:
            for address in [network.health.address, network.a.address]:
                conn = (socket.create_connection(address, timeout=2) if address == network.health.address
                        else fixtures.socks_connect(address, "127.0.0.1", network.health.address[1]))
                with conn:
                    conn.sendall(b"GET /health HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
                    self.assertTrue(conn.recv(4096).startswith(b"HTTP/1.1 204"))
            self.assertEqual(network.snapshot()["a"]["targets"][0]["kind"], "health")

    def test_configuration_preserves_dashboard_default_and_changes_graph_copy(self):
        with fixtures.Network() as network:
            patch = network.config_patch(39901, 39902, "private-package-test-secret")
            self.assertNotIn("singboxDashboard", patch)
            self.assertEqual(patch["proxyModeType"], "manual")
            changed = network.changed_graph({**patch, "singboxDashboard": True})
            self.assertTrue(changed["singboxDashboard"])
            self.assertEqual(patch["trafficRules"][0]["targetServerId"], "node-a")
            for key in ["trafficRules", "policyRules", "customRules"]:
                self.assertEqual(changed[key][0]["targetServerId"], "node-b")
            self.assertEqual(changed["dnsRules"][0]["effects"]["dns"]["action"]["serverId"], "loop-b")
            with self.assertRaises(ValueError):
                network.config_patch(39901, 39901, "private-package-test-secret")
            with self.assertRaises(ValueError):
                network.config_patch(39901, 39902, "short")

    def test_invalid_endpoint_and_latency_rejected(self):
        with self.assertRaises(ValueError):
            fixtures.socks_connect(("example.org", 80))
        with self.assertRaises(ValueError):
            fixtures.Resolver("8.8.8.8")
        backend = fixtures.SocksBackend("B")
        try:
            for value in [-1, 3, "1"]:
                with self.assertRaises(ValueError):
                    backend.set_latency(value)
            with self.assertRaises(ValueError):
                backend.set_admission(0)
        finally:
            backend.close()


class ProbeSourceContracts(unittest.TestCase):
    def test_probe_is_test_only_and_all_six_methods_are_authenticated(self):
        root = ROOT.parents[1]
        source = (root / "crates/singbox-grpc/examples/package_runtime_probe.rs").read_text()
        self.assertIn("std::io::stdin()", source)
        self.assertIn("std::env::args_os().len() == 1", source)
        self.assertIn("listener-not-owned-by-core", source)
        self.assertIn("owned-process-changed", source)
        self.assertIn("runtime-content-differs", source)
        for method in ["get_runtime_capabilities", "plan_runtime_config", "apply_runtime_config",
                       "get_runtime_config_status", "retry_runtime_persistence", "acknowledge_runtime_persistence"]:
            self.assertRegex(source, r"unauth\s*\.\s*" + method)
        body = source.split("async fn apply_once(")[1].split("async fn suite(")[0]
        self.assertEqual(body.count("client.apply_runtime_config(request)"), 1)
        self.assertIn("client.get_runtime_config_status(lookup.clone())", re.sub(r"\s+", "", body))
        self.assertIn('"resubmitted":false', body)
        self.assertNotIn("Command::new", source)
        self.assertNotIn("eprintln!", source)
        self.assertIn('"positiveRegenerationAck":"not covered; requires real libbox owner"', source)


if __name__ == "__main__":
    unittest.main(verbosity=2)
