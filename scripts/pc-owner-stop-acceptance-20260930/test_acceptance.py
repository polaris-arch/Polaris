"""Harmless files only: forged envelopes must never become device acceptance."""

import argparse
import copy
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

SCRIPT = Path(__file__).with_name("acceptance.py")
SPEC = importlib.util.spec_from_file_location("owner_stop_acceptance", SCRIPT)
qa = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(qa)
# An explicit historical source binding is harmless fixture data, never a CLI default.
FIXTURE_CANDIDATE = "7253f0b06d88d54ca6e104320459944a5b6a14c0"


class AcceptanceTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.report = {
            "schemaVersion": 1, "kind": "READ_ONLY_PREFLIGHT",
            "binding": {"candidateSha": FIXTURE_CANDIDATE, "deviceId": "fixture-PC_01", "nonce": "ab" * 16, "platform": "linux"},
            "artifacts": {role: {"sha256": str(index + 1) * 64, "bytes": 10} for index, role in enumerate(qa.ROLES)},
            "versions": {"app": "fixture+1", "helper": "fixture+1", "core": "fixture+1", "source": "operator-declared-unverified"},
            "host": {"system": "Linux", "release": "fixture", "architecture": "fixture", "hostnameSha256": "d" * 64},
            "readOnly": True, "capabilities": dict(qa.CAPABILITIES), "claims": [],
        }
        self.plan = qa.prepare(self.report)
        self.witness = {
            "schemaVersion": 1, "kind": "OFFLINE_EVIDENCE_BUNDLE", "synthetic": False,
            "binding": copy.deepcopy(self.plan["binding"]), "planSha256": self.plan["planSha256"],
            "artifacts": copy.deepcopy(self.plan["artifacts"]), "versions": dict(self.plan["versions"]),
            "records": [{"caseId": phase["id"], "state": "UNKNOWN", "capture": None} for phase in self.plan["phases"]],
            "claims": [],
        }

    def write_json(self, name, value):
        path = self.root / name
        path.write_bytes(qa.canonical(value))
        return path

    def reference(self, path):
        return {"path": path.relative_to(self.root).as_posix(), "sha256": qa.file_digest(path), "bytes": path.stat().st_size}

    def add_capture(self, index, state="OBSERVED"):
        # These are handwritten test shapes, not a runtime exporter or native proof.
        phase = self.plan["phases"][index]
        original = self.root / f"original-{index}.txt"
        original.write_text("UNTRUSTED FIXTURE; no native process was run\n", encoding="utf-8")
        capture = {
            "synthetic": False, "binding": copy.deepcopy(self.plan["binding"]),
            "planSha256": self.plan["planSha256"], "caseId": phase["id"], "state": state,
            "source": phase["requiredSource"],
            "identities": {producer: {"generation": "fixture-generation", "nativeIdentity": "opaque-untrusted-fixture"} for producer in phase["producers"]},
            "original": self.reference(original),
        }
        self.save_capture(index, capture)
        self.witness["records"][index]["state"] = state
        return capture

    def save_capture(self, index, capture):
        path = self.write_json(f"capture-{index}.json", capture)
        self.witness["records"][index]["capture"] = self.reference(path)

    def verify(self):
        return qa.verify_witness(self.plan, self.witness, self.root)

    def cli(self, *arguments):
        return subprocess.run([sys.executable, str(SCRIPT), *map(str, arguments)], capture_output=True, text=True,
                              env={**os.environ, "PYTHONDONTWRITEBYTECODE": "1"}, check=False)

    def test_unknown_and_pending_preserve_typed_missing_capabilities(self):
        self.witness["records"][1]["state"] = "PENDING"
        result = self.verify()
        self.assertEqual(result["verdict"], "NOT_READY")
        self.assertEqual(len(result["retainedStates"]), len(qa.CASES))
        self.assertEqual(result["retainedStates"][1]["state"], "PENDING")
        self.assertEqual(result["claims"], [])
        self.assertIn("trusted-native-receipt-exporter", result["missingCapabilities"])
        self.assertEqual(self.plan["capabilities"], qa.CAPABILITIES)

    def test_complete_handwritten_envelopes_still_never_accept_device(self):
        for index in range(len(qa.CASES)):
            self.add_capture(index)
        result = self.verify()
        self.assertTrue(result["witnessConsistent"])
        self.assertEqual(result["verdict"], "NOT_READY")
        self.assertEqual(result["claims"], [])
        self.assertIn("artifact-build-provenance", result["missingCapabilities"])
        plan_file = self.write_json("plan.json", self.plan)
        witness_file = self.write_json("witness.json", self.witness)
        outcome = self.cli("verify", plan_file, witness_file, "--bundle", self.root)
        self.assertEqual(outcome.returncode, 2, outcome.stderr)
        self.assertEqual(json.loads(outcome.stdout)["verdict"], "NOT_READY")

    def test_prepare_exit_two_and_no_repeat_approval_requirement(self):
        result = self.cli("prepare", self.write_json("preflight.json", self.report))
        self.assertEqual(result.returncode, 2, result.stderr)
        plan = json.loads(result.stdout)
        self.assertTrue(plan["preparedOnly"])
        self.assertEqual(plan["windowRequest"]["authorization"], "EXISTING_SESSION_SCOPE_ONLY")
        self.assertNotIn("after explicit approval", result.stdout)

    def test_prepare_does_not_share_mutable_contract_or_report(self):
        self.plan["phases"][1]["producers"].clear()
        self.plan["windowRequest"]["scopes"].append("new-authority")
        self.plan["binding"]["nonce"] = "cd" * 16
        self.assertEqual(qa.phases()[1]["producers"], ["main"])
        self.assertNotIn("new-authority", qa.WINDOW["scopes"])
        self.assertEqual(self.report["binding"]["nonce"], "ab" * 16)

    def test_plan_tamper_or_resource_claim_is_rejected_even_after_rehash(self):
        for field, replacement in (("claims", ["globalNoOwner"]), ("capabilities", {}), ("phases", []), ("windowRequest", {"authorization": "GRANTED"})):
            with self.subTest(field=field):
                plan = copy.deepcopy(self.plan)
                plan[field] = replacement
                plan["planSha256"] = qa.digest({key: value for key, value in plan.items() if key != "planSha256"})
                with self.assertRaises(ValueError):
                    qa.validate_plan(plan)
        self.plan["planSha256"] = "0" * 64
        with self.assertRaises(ValueError):
            self.verify()

    def test_missing_extra_duplicate_or_nonobject_records_are_rejected(self):
        original = copy.deepcopy(self.witness["records"])
        malformed = (original[:-1], original + [None], original + [original[0]], [None] + original[1:], [original[1], original[0]] + original[2:])
        for records in malformed:
            with self.subTest(records=len(records)):
                self.witness["records"] = records
                with self.assertRaises(ValueError):
                    self.verify()

    def test_old_wrong_candidate_device_nonce_artifact_and_version_are_rejected(self):
        for field, value in (("candidateSha", "0" * 40), ("deviceId", "other-device"), ("nonce", "cd" * 16), ("platform", "macos")):
            with self.subTest(field=field):
                bad = copy.deepcopy(self.witness)
                bad["binding"][field] = value
                with self.assertRaises(ValueError):
                    qa.verify_witness(self.plan, bad, self.root)
        for field in ("artifacts", "versions", "planSha256"):
            bad = copy.deepcopy(self.witness)
            bad[field] = {} if field != "planSha256" else "0" * 64
            with self.assertRaises(ValueError):
                qa.verify_witness(self.plan, bad, self.root)

    def test_synthetic_witness_and_capture_are_rejected(self):
        self.witness["synthetic"] = True
        with self.assertRaises(ValueError):
            self.verify()
        self.witness["synthetic"] = False
        capture = self.add_capture(0)
        capture["synthetic"] = True
        self.save_capture(0, capture)
        with self.assertRaises(ValueError):
            self.verify()

    def test_observed_case_without_capture_cannot_use_ack_or_pid_list(self):
        self.witness["records"][0]["state"] = "OBSERVED"
        with self.assertRaises(ValueError):
            self.verify()
        capture = self.add_capture(3)
        self.witness["records"][0]["state"] = "UNKNOWN"
        for source in ("Stop ACK", "PID list"):
            capture["source"] = source
            self.save_capture(3, capture)
            with self.assertRaises(ValueError):
                self.verify()

    def test_unknown_capture_cannot_be_reclassified_as_observed(self):
        self.add_capture(3, "UNKNOWN")
        self.witness["records"][3]["state"] = "OBSERVED"
        with self.assertRaisesRegex(ValueError, "cannot be reclassified"):
            self.verify()

    def test_old_capture_binding_or_missing_same_generation_is_rejected(self):
        capture = self.add_capture(3)
        capture["binding"]["nonce"] = "cd" * 16
        self.save_capture(3, capture)
        with self.assertRaises(ValueError):
            self.verify()
        capture["binding"] = copy.deepcopy(self.plan["binding"])
        capture["identities"] = {}
        self.save_capture(3, capture)
        with self.assertRaises(ValueError):
            self.verify()

    def test_original_hash_size_and_nonempty_receipt_required(self):
        capture = self.add_capture(0)
        original = self.root / capture["original"]["path"]
        original.write_bytes(b"corrupted bytes")
        with self.assertRaises(ValueError):
            self.verify()
        original.write_bytes(b"")
        capture["original"] = self.reference(original)
        self.save_capture(0, capture)
        with self.assertRaisesRegex(ValueError, "nonempty evidence"):
            self.verify()

    def test_path_escape_and_symlink_receipts_are_rejected(self):
        path = self.root / "real.txt"
        path.write_bytes(b"fixture")
        reference = self.reference(path)
        for name in ("../real.txt", str(path)):
            with self.subTest(name=name):
                reference["path"] = name
                with self.assertRaises(ValueError):
                    qa.checked_file(self.root, reference)
        link = self.root / "linked.txt"
        try:
            link.symlink_to(path)
        except OSError:
            self.skipTest("symlinks unavailable on this fixture host")
        reference["path"] = link.name
        with self.assertRaises(ValueError):
            qa.checked_file(self.root, reference)

    def test_bounded_json_and_duplicate_keys_fail_closed(self):
        path = self.root / "oversized.json"
        path.write_bytes(b" " * (qa.MAX_JSON_BYTES + 1))
        with self.assertRaisesRegex(ValueError, "1 MiB"):
            qa.load(path)
        path.write_text('{"secret-field":1,"secret-field":2}', encoding="utf-8")
        with self.assertRaisesRegex(ValueError, "duplicate JSON field"):
            qa.load(path)
        result = self.cli("prepare", path)
        self.assertEqual(result.returncode, 1)
        self.assertNotIn("secret-field", result.stderr)

    def test_fixture_preflight_only_hashes_roles_and_redacts_host_and_paths(self):
        labels = {}
        args = argparse.Namespace(platform="linux", candidate=FIXTURE_CANDIDATE, device_id="fixture-PC_01", nonce="ab" * 16)
        for role in qa.ROLES:
            file = self.root / f"private-{role}.txt"
            file.write_bytes(b"ordinary fixture bytes, never executable")
            setattr(args, role, str(file))
            setattr(args, role + "_version", "fixture+1")
            labels[role] = qa.file_digest(file)
        with mock.patch.multiple(qa.platform, system=lambda: "Linux", node=lambda: "PRIVATE-HOST", release=lambda: "fixture", machine=lambda: "fixture"):
            report = qa.preflight(args)
        encoded = json.dumps(report)
        self.assertNotIn(str(self.root), encoded)
        self.assertNotIn("PRIVATE-HOST", encoded)
        self.assertEqual(report["capabilities"], qa.CAPABILITIES)
        self.assertEqual({role: item["sha256"] for role, item in report["artifacts"].items()}, labels)
        for role in qa.ROLES:
            self.assertEqual(set(report["artifacts"][role]), {"sha256", "bytes"})
        args.platform = "macos"
        with mock.patch.object(qa.platform, "system", return_value="Linux"), self.assertRaises(ValueError):
            qa.preflight(args)

    def test_device_id_and_versions_constraints_are_distinct(self):
        self.report["binding"]["deviceId"] = "device+secret"
        with self.assertRaises(ValueError):
            qa.prepare(self.report)
        ps = SCRIPT.with_name("preflight-windows.ps1").read_text(encoding="utf-8")
        self.assertIn("$DeviceId -cnotmatch '^[A-Za-z0-9._-]{1,96}$'", ps)
        self.assertIn("[PlatformID]::Win32NT", ps)
        self.assertIn("'64bit-os'", ps)
        self.assertIn("'32bit-os'", ps)

    def test_rejected_cli_does_not_echo_paths_or_unknown_arguments(self):
        outcome = self.cli("prepare", self.root / "PRIVATE-TOKEN-missing.json")
        self.assertEqual(outcome.returncode, 1)
        self.assertNotIn("PRIVATE-TOKEN", outcome.stderr)
        outcome = self.cli("--execute=PRIVATE-TOKEN")
        self.assertEqual(outcome.returncode, 1)
        self.assertNotIn("PRIVATE-TOKEN", outcome.stderr)

    def test_preflight_requires_explicit_candidate_before_reading_files(self):
        args = ["--device-id", "fixture-PC_01", "--nonce", "ab" * 16]
        for role in qa.ROLES:
            args.extend(["--" + role, "unused-fixture-path", "--" + role + "-version", "fixture"])
        for platform in ("linux", "macos", "windows"):
            with self.subTest(platform=platform):
                outcome = self.cli("preflight", "--platform", platform, *args)
                self.assertEqual(outcome.returncode, 1)
                self.assertEqual(outcome.stdout, "")
                self.assertIn("invalid arguments", outcome.stderr)
                self.assertNotIn("artifact IO", outcome.stderr)
        ps = SCRIPT.with_name("preflight-windows.ps1").read_text(encoding="utf-8")
        self.assertIn("[Parameter(Mandatory=$true)][string]$Candidate,", ps)
        self.assertNotIn(FIXTURE_CANDIDATE, ps)

    def test_receipt_claims_and_inferred_preflight_capabilities_are_rejected(self):
        self.witness["claims"] = ["nativeStopped", "NoOwner", "managed", "releaseCleared"]
        with self.assertRaises(ValueError):
            self.verify()
        self.report["capabilities"]["helperProtocolObserved"] = "SUPPORTED"
        with self.assertRaises(ValueError):
            qa.prepare(self.report)


if __name__ == "__main__":
    unittest.main()
