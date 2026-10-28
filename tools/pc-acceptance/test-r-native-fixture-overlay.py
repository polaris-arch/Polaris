#!/usr/bin/env python3
"""Pure source/patch checks. These do not execute Go or grant native admission."""

import hashlib
from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[2]
CANONICAL = ROOT / "scripts/libbox-patches/sing-tun-owned-native.patch"
OVERLAY = Path(__file__).with_name("r-native-fixture-overlay.patch")
TARGET = "route_native_receipt_linux_test.go"
BEFORE_SHA256 = "da784771f37e2a022afd3d6f333c1a8fc4cc2a0ff8c2e6401fba00bf6ba1be34"


def original_fixture(container):
    marker = f"diff --git a/{TARGET} b/{TARGET}\n".encode()
    if container.count(marker) != 1:
        raise ValueError("original fixture section is not unique")
    section = container.split(marker, 1)[1]
    boundary = section.find(b"\ndiff --git ")
    if boundary != -1:
        section = section[:boundary + 1]  # The added final line owns this LF.
    source = b"".join(line[1:] for line in section.splitlines(keepends=True)
                      if line.startswith(b"+") and not line.startswith(b"+++"))
    if hashlib.sha256(source).hexdigest() != BEFORE_SHA256:
        raise ValueError("original fixture preimage differs")
    return source


def function(source, name):
    # These existing gofmt functions end at a column-zero brace. Select only
    # the named function, never a whole-file positive/negative text search.
    marker = f"\nfunc {name}(".encode()
    if source.count(marker) != 1:
        raise ValueError("source function is not unique")
    start = source.index(marker) + 1
    end = source.index(b"\n}\n", start) + 2
    return source[start:end]


def binding_contract(source):
    body = function(source, "rNativeLauncherBinding")
    actual = [line.strip() for line in body.decode().splitlines()
              if line.strip() and not line.strip().startswith("//")]
    expected = [
        "func rNativeLauncherBinding(c rNativeCase) (*rNativeLauncherInputs, error) {",
        'if err := rNativeAdmission(os.Getenv("POLARIS_NO_KERNEL_RUN"), os.Getenv("POLARIS_R_NATIVE_RUN"), nil); err != nil {',
        "return nil, err",
        "}",
        "return rNativeConsumeFixedFDGraph(c)",
        "}",
    ]
    if actual != expected:
        raise ValueError("binding must admit before consuming the original FD graph")


def git_apply(directory, patch, *flags):
    result = subprocess.run(
        ["git", "apply", *flags, str(patch)], cwd=directory,
        capture_output=True, timeout=5, check=False,
    )
    if result.returncode:
        raise AssertionError(result.stderr.decode(errors="replace"))


class RFixtureOverlayTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.before = original_fixture(CANONICAL.read_bytes())
        cls.patch = OVERLAY.read_bytes()
        with tempfile.TemporaryDirectory(prefix="polaris-r-overlay-pure-") as directory:
            target = Path(directory) / TARGET
            target.write_bytes(cls.before)
            git_apply(directory, OVERLAY, "--check")
            git_apply(directory, OVERLAY)
            cls.after = target.read_bytes()
            git_apply(directory, OVERLAY, "--reverse", "--check")
            git_apply(directory, OVERLAY, "--reverse")
            cls.reversed = target.read_bytes()

    def test_actual_patch_and_inverse_restore_exact_original_bytes(self):
        self.assertNotEqual(self.after, self.before)
        self.assertEqual(self.reversed, self.before)
        self.assertEqual(hashlib.sha256(self.reversed).hexdigest(), BEFORE_SHA256)
        self.assertTrue(self.before.endswith(b"\n"))
        self.assertTrue(self.after.endswith(b"\n"))
        self.assertEqual(len(self.before), 60675)
        self.assertEqual(len(self.after), 61263)
        self.assertEqual(hashlib.sha256(self.after).hexdigest(),
                         "0ab03175b50ede1f46124718332a3cc367f2bb294d842abf874568e37a7b015e")
        self.assertNotEqual(hashlib.sha256(self.before[:-1]).hexdigest(), BEFORE_SHA256)

    def test_patch_has_exactly_one_original_logical_file(self):
        headers = [line for line in self.patch.splitlines()
                   if line.startswith((b"diff --git ", b"--- ", b"+++ "))]
        self.assertEqual(headers, [
            f"diff --git a/{TARGET} b/{TARGET}".encode(),
            f"--- a/{TARGET}".encode(), f"+++ b/{TARGET}".encode(),
        ])

    def test_all_bytes_outside_binding_and_one_existing_test_are_preserved(self):
        masked = []
        for source in (self.before, self.after):
            for name in ("rNativeLauncherBinding", "TestRNativeProfileNoKernelRunWins"):
                source = source.replace(function(source, name), name.encode(), 1)
            masked.append(source)
        self.assertEqual(*masked)

    def test_exact_admission_consumer_body_rejects_source_order_and_authority_mutants(self):
        binding_contract(self.after)
        guard = b'if err := rNativeAdmission(os.Getenv("POLARIS_NO_KERNEL_RUN"), os.Getenv("POLARIS_R_NATIVE_RUN"), nil); err != nil {'
        mutations = (
            (guard, guard.replace(b"err != nil", b"err == nil")),
            (guard, guard.replace(b'os.Getenv("POLARIS_NO_KERNEL_RUN")', b'""')),
            (guard, guard.replace(b'os.Getenv("POLARIS_R_NATIVE_RUN")', b'"1"')),
            (guard, b"rNativeConsumeFixedFDGraph(c)\n\t" + guard),
            (b"return nil, err", b"return nil, nil"),
            (b"return rNativeConsumeFixedFDGraph(c)", b"return nil, errRNativeGSeamPending"),
            (b"return rNativeConsumeFixedFDGraph(c)", b"return rNativeConsumeFixedFDGraph(rNativeCase{})"),
        )
        original = function(self.after, "rNativeLauncherBinding")
        for old, new in mutations:
            with self.subTest(mutant=new):
                self.assertEqual(original.count(old), 1)
                changed = self.after.replace(original, original.replace(old, new, 1), 1)
                with self.assertRaises(ValueError):
                    binding_contract(changed)

    def test_original_three_assertions_and_direct_deny_optin_criteria_are_kept(self):
        before = function(self.before, "TestRNativeProfileNoKernelRunWins")
        after = function(self.after, "TestRNativeProfileNoKernelRunWins")
        self.assertEqual(before.split(b"\t// Binding itself", 1)[0],
                         after.split(b"\t// The direct binding", 1)[0])
        for criterion in (
            b't.Setenv("POLARIS_R_NATIVE_RUN", "1")',
            b'for _, deny := range []string{"1", "0"}',
            b't.Setenv("POLARIS_NO_KERNEL_RUN", deny)',
            b'input != nil || err == nil || !strings.Contains(err.Error(), "NO_KERNEL_RUN")',
            b't.Setenv("POLARIS_NO_KERNEL_RUN", "")',
            b't.Setenv("POLARIS_R_NATIVE_RUN", "")',
            b'input != nil || err == nil || !strings.Contains(err.Error(), "opt-in")',
        ):
            self.assertEqual(after.count(criterion), 1)

    def test_original_caller_admission_and_selector_still_precede_binding(self):
        marker = b"diff --git a/route_native_kernel_linux_test.go b/route_native_kernel_linux_test.go\n"
        section = CANONICAL.read_bytes().split(marker, 1)[1].split(b"\ndiff --git ", 1)[0]
        source = b"".join(line[1:] for line in section.splitlines(keepends=True)
                          if line.startswith(b"+") and not line.startswith(b"+++"))
        caller = function(source, "rNativeRunGroup")
        self.assertLess(caller.index(b"rNativeAdmission("), caller.index(b"rNativeValidateSelector("))
        self.assertLess(caller.index(b"rNativeValidateSelector("), caller.index(b"rNativeLauncherBinding(c)"))
        self.assertLess(caller.index(b"rNativeLauncherBinding(c)"), caller.index(b"rNativeVerifyEnvironment(inputs, c)"))
        self.assertLess(caller.index(b"rNativeVerifyEnvironment(inputs, c)"), caller.index(b"rNativeRunCase(t, c, inputs, deadline)"))

    def test_old_or_altered_fixture_preimage_is_rejected(self):
        original = CANONICAL.read_bytes()
        for changed in (original.replace(b"rNativeLauncherBinding(rNativeCase)", b"rNativeLauncherBinding(otherCase)", 1),
                        original + f"diff --git a/{TARGET} b/{TARGET}\n".encode()):
            with self.subTest(kind="altered or repeated original"):
                with self.assertRaises(ValueError):
                    original_fixture(changed)


if __name__ == "__main__":
    unittest.main()
