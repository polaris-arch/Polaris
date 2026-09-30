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


if __name__ == "__main__":
    unittest.main()
