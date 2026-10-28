#!/usr/bin/env python3
"""Exercise actual patch replay, module selection and ELF provenance offline."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("source_provision", Path(__file__).with_name("core-source-provision.py"))
provider = importlib.util.module_from_spec(spec)
spec.loader.exec_module(provider)
GO = Path.home() / "go/pkg/mod/golang.org/toolchain@v0.0.1-go1.25.5.linux-amd64/bin/go"


def git(path, *args):
    return subprocess.check_output(["git", "-C", str(path), *args], text=True).strip()


def commit(path, message):
    git(path, "add", "-A")
    git(path, "-c", "user.name=Source fixture", "-c", "user.email=source-fixture@example.invalid", "-c", "commit.gpgsign=false", "commit", "-m", message)
    return git(path, "rev-parse", "HEAD")


class SourceProvisionTests(unittest.TestCase):
    def test_replay_selects_patched_module_and_binds_binary(self):
        # Each repository is a flat ~/Code fixture, not a nested source clone.
        with tempfile.TemporaryDirectory(prefix="polaris-source-fixture-core-", dir=Path.home()/"Code") as core_name, tempfile.TemporaryDirectory(prefix="polaris-source-fixture-dep-", dir=Path.home()/"Code") as dependency_name, tempfile.TemporaryDirectory(prefix="polaris-source-fixture-out-", dir=Path.home()/"Code") as output_name:
            core, dependency, output = map(Path, (core_name, dependency_name, output_name))
            git(core, "init", "-q")
            git(dependency, "init", "-q")
            # Deliberately unavailable optional metadata must not expand the
            # declared patched-module query into an all-module graph assertion.
            (core / "go.mod").write_text("module example.org/core-fixture\n\ngo 1.25.5\n\nrequire (\n example.org/patched v0.1.0\n example.org/unavailable v1.0.0\n)\n")
            (core / "go.sum").write_text("")
            (core / "main.go").write_text('package main\nimport ("fmt"; patched "example.org/patched")\nfunc main(){fmt.Println(patched.Answer())}\n')
            core_commit = commit(core, "core upstream fixture")
            (dependency / "go.mod").write_text("module example.org/patched\n\ngo 1.25.5\n")
            (dependency / "answer.go").write_text("package patched\nfunc Answer() int { return 1 }\n")
            dependency_commit = commit(dependency, "dependency upstream fixture")
            (dependency / "answer.go").write_text("package patched\nfunc Answer() int { return 2 }\n")
            patch = subprocess.check_output(["git", "-C", str(dependency), "diff"])
            commit(dependency, "dependency patched fixture")
            patched_tree = git(dependency, "rev-parse", "HEAD^{tree}")
            patch_file = core / "dependency.patch"
            patch_file.write_bytes(patch)
            manifest = {"sourceCommit": core_commit, "goVersion": "1.25.5", "patches": [],
                        "dependencyPatches": [{"name": "fixture", "module": "example.org/patched", "upstreamVersion": "v0.1.0", "upstreamCommit": dependency_commit, "sourceURL": "https://github.com/example/fixture", "patchFile": patch_file.name, "patchSha256": hashlib.sha256(patch).hexdigest(), "patchedTree": patched_tree}]}
            manifest_file = core / "manifest.json"
            manifest_file.write_text(json.dumps(manifest))
            receipt = provider.provision(manifest_file, core, output, {"example.org/patched": dependency}, GO)
            self.assertIn("return 2", (output / "polaris-dependencies/fixture/answer.go").read_text())
            self.assertEqual(receipt["dependencies"][0]["patchedTree"], patched_tree)
            self.assertIn("replace example.org/patched => ./polaris-dependencies/fixture", (output / "go.mod").read_text())
            self.assertEqual(receipt["graphScope"], "declared-patched-modules")
            self.assertEqual(receipt["moduleGraphQueries"], ["example.org/patched"])
            self.assertEqual([item["Path"] for item in receipt["moduleGraph"]], ["example.org/patched"])
            self.assertEqual(receipt["mainGoModSha256"], hashlib.sha256((output / "go.mod").read_bytes()).hexdigest())
            self.assertEqual(receipt["mainGoSumSha256"], hashlib.sha256((output / "go.sum").read_bytes()).hexdigest())
            provider.verify_checkout(output, receipt)
            environment = os.environ | {"GOWORK": "off", "GOTOOLCHAIN": "local", "GOPROXY": "off", "GOSUMDB": "off", "GOFLAGS": ""}
            binary = output / "fixture.elf"
            subprocess.run([str(GO), "build", "-ldflags="+provider.source_linker_flag(receipt), "-o", str(binary), "."], cwd=output, env=environment, check=True)
            binary_receipt = provider.verify_binary(GO, binary, receipt, ("example.org/patched",))
            self.assertEqual(binary_receipt["sourceFingerprint"], receipt["fingerprint"])
            provider.verify_checkout(output, receipt)
            (output / "extra.go").write_text("package main\n")
            with self.assertRaisesRegex(RuntimeError, "untracked compiler input"):
                provider.verify_checkout(output, receipt)
            manifest["dependencyPatches"][0]["patchSha256"] = "0"*64
            manifest_file.write_text(json.dumps(manifest))
            with tempfile.TemporaryDirectory(prefix="polaris-source-fixture-reject-", dir=Path.home()/"Code") as rejected_name:
                with self.assertRaisesRegex(RuntimeError, "patch hash mismatch"):
                    provider.provision(manifest_file, core, Path(rejected_name), {"example.org/patched": dependency}, GO)
                self.assertEqual(list(Path(rejected_name).iterdir()), [])


if __name__ == "__main__":
    unittest.main()
