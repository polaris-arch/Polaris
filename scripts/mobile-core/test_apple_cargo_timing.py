"""Bounded Cargo diagnostics only; no Cargo, Xcode or native compiler runs."""
import ast
from contextlib import nullcontext
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest import mock
import zipfile

import apple_rust
import ci_apple


class AppleCargoTimingTests(unittest.TestCase):
    triple = 'aarch64-apple-ios'

    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = Path(temporary.name).resolve()
        self.target = self.root / 'cache/rust-target'
        self.report = self.target / 'cargo-timings/cargo-timing.html'
        self.report.parent.mkdir(parents=True)
        (self.root / 'cache').chmod(0o700)
        self.work = self.root / 'device-arm64'
        self.work.mkdir()
        self.output = self.work / ('diagnostic-cargo-timing-' + self.triple + '.html')
        printer = mock.patch('builtins.print')
        self.printed = printer.start()
        self.addCleanup(printer.stop)

    def observe(self):
        return apple_rust.cargo_timing_report(self.target, self.work, self.triple)

    def test_single_report_keeps_exact_bytes_and_ignores_history(self):
        history = self.report.with_name('cargo-timing-old.html')
        history.write_bytes(b'old unrelated history')
        with self.observe():
            self.report.write_bytes(b'<html>actual units</html>')
        self.assertEqual(self.output.read_bytes(), self.report.read_bytes())
        self.assertEqual(list(self.work.iterdir()), [self.output])
        self.assertEqual(history.read_bytes(), b'old unrelated history')
        self.printed.assert_not_called()

    def test_failed_cargo_retains_report_without_replacing_primary_exception(self):
        primary = RuntimeError('original cargo failure')
        with self.assertRaises(RuntimeError) as caught:
            with self.observe():
                self.report.write_bytes(b'<html>partial build</html>')
                raise primary
        self.assertIs(caught.exception, primary)
        self.assertEqual(self.output.read_bytes(), self.report.read_bytes())

    def test_missing_or_unchanged_report_cannot_impersonate_current_slot(self):
        for previous in (None, b'<html>previous slot</html>'):
            with self.subTest(previous=previous):
                if previous is not None:
                    self.report.write_bytes(previous)
                with self.observe():
                    pass
                self.assertFalse(self.output.exists())

    def test_invalid_size_is_diagnostic_only_on_success_and_failure(self):
        for raw in (b'', b'12345'):
            for failed in (False, True):
                with self.subTest(raw=raw, failed=failed), \
                        mock.patch.object(apple_rust, 'CARGO_TIMING_BYTES', 4):
                    self.report.unlink(missing_ok=True)
                    primary = RuntimeError('original failure')
                    try:
                        with self.observe():
                            self.report.write_bytes(raw)
                            if failed:
                                raise primary
                    except RuntimeError as caught:
                        self.assertIs(caught, primary)
                    self.assertFalse(self.output.exists())

    def test_report_file_and_parent_redirects_are_not_copied(self):
        external = self.root / 'outside.html'
        external.write_bytes(b'do not copy')
        with self.observe():
            self.report.symlink_to(external)
        self.assertFalse(self.output.exists())
        self.report.unlink()
        self.report.parent.rmdir()
        external_dir = self.root / 'outside-dir'
        external_dir.mkdir()
        self.report.parent.symlink_to(external_dir, target_is_directory=True)
        with self.observe():
            self.report.write_bytes(b'do not copy parent redirect')
        self.assertFalse(self.output.exists())

    def test_existing_diagnostic_is_never_overwritten(self):
        self.output.write_bytes(b'prior retained diagnostic')
        with self.observe():
            self.report.write_bytes(b'new report')
        self.assertEqual(self.output.read_bytes(), b'prior retained diagnostic')

    def test_exact_bound_allowed_and_copy_failure_preserves_cargo_error(self):
        with mock.patch.object(apple_rust, 'CARGO_TIMING_BYTES', 4), self.observe():
            self.report.write_bytes(b'1234')
        self.assertEqual(self.output.read_bytes(), b'1234')
        primary = RuntimeError('original failure')
        with self.assertRaises(RuntimeError) as caught:
            with self.observe():
                self.report.write_bytes(b'changed report')
                raise primary
        self.assertIs(caught.exception, primary)

    def test_existing_formal_cargo_command_adds_one_timing_flag_without_rerun(self):
        tree = ast.parse(Path(apple_rust.__file__).read_text())
        build = next(node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name == 'build_reserved')
        commands = [node.value for node in ast.walk(build) if isinstance(node, ast.Assign) and
                    any(isinstance(target, ast.Name) and target.id == 'command' for target in node.targets)]
        self.assertEqual(len(commands), 1)
        flags = [item.value for item in commands[0].elts if isinstance(item, ast.Constant)]
        self.assertEqual(flags, ['rustc', '--locked', '--release', '--timings', '--lib', '--package',
                                 'polaris', '--target', '--features', 'tauri/custom-protocol',
                                 '--crate-type', 'staticlib'])
        calls = [node for node in ast.walk(build) if isinstance(node, ast.Call) and
                 isinstance(node.func, ast.Name) and node.func.id == 'run' and node.args and
                 isinstance(node.args[0], ast.Name) and node.args[0].id == 'command']
        self.assertEqual(len(calls), 1)

    def test_originals_zip_selects_one_bounded_diagnostic_without_acceptance(self):
        self.output.write_bytes(b'<html>one report</html>')
        (self.work / 'diagnostic-cargo-timing-history.html').write_bytes(b'history')
        self.report.write_bytes(b'not selected from shared cache')
        with mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': 'device-only'}, clear=True), \
                mock.patch.object(ci_apple, 'location', return_value=self.root), \
                mock.patch.object(ci_apple, 'lease', side_effect=lambda *args, **kwargs: nullcontext()):
            ci_apple.evidence()
        with zipfile.ZipFile(self.root / 'apple-original-evidence.zip') as package:
            html = [name for name in package.namelist() if name.endswith('.html')]
            self.assertEqual(html, [self.output.relative_to(self.root).as_posix()])
            self.assertEqual(package.read(html[0]), self.output.read_bytes())
            index = json.loads(package.read('EVIDENCE-INDEX.json'))
            self.assertFalse(index['componentAccepted'])
            self.assertFalse(index['completeUnsignedChain'])
            self.assertFalse(index['devicePassed'])
            self.assertFalse(index['publicationEligible'])

    def preserve_originals(self):
        with mock.patch.dict(os.environ, {'POLARIS_APPLE_SCOPE': 'device-only'}, clear=True), \
                mock.patch.object(ci_apple, 'location', return_value=self.root), \
                mock.patch.object(ci_apple, 'lease', side_effect=lambda *args, **kwargs: nullcontext()):
            ci_apple.evidence()
        path = self.root / 'apple-original-evidence.zip'
        with zipfile.ZipFile(path) as package:
            members = {name: package.read(name) for name in package.namelist()}
        size = path.stat().st_size
        path.unlink()
        return members, size

    def test_diagnostic_cannot_consume_mandatory_entry_content_index_path_or_zip_budget(self):
        mandatory = self.root / 'mandatory.log'
        mandatory.write_bytes(b'original mandatory bytes')
        before, zip_size = self.preserve_originals()
        index_size = len(before['EVIDENCE-INDEX.json'])
        self.output.write_bytes(b'<html>optional diagnostic</html>')
        limits = {'EVIDENCE_ENTRIES': 1, 'EVIDENCE_CONTENT_BYTES': mandatory.stat().st_size,
                  'EVIDENCE_INDEX_BYTES': index_size, 'EVIDENCE_PATH_BYTES': len(mandatory.name),
                  'EVIDENCE_ZIP_BYTES': zip_size}
        for constant, value in limits.items():
            with self.subTest(budget=constant), mock.patch.object(ci_apple, constant, value):
                after, _ = self.preserve_originals()
            self.assertEqual(after, before)
        # With ordinary remaining budgets, exactly the optional member is added.
        after, _ = self.preserve_originals()
        name = self.output.relative_to(self.root).as_posix()
        self.assertEqual(set(after), set(before) | {name})
        self.assertEqual(after['mandatory.log'], before['mandatory.log'])
        index = json.loads(after['EVIDENCE-INDEX.json'])
        self.assertTrue(index['files'][name]['diagnosticOnly'])
        self.assertEqual(index['receiptReadErrors'], {})

    def test_optional_read_or_validation_errors_preserve_identical_mandatory_result(self):
        (self.root / 'mandatory.log').write_bytes(b'mandatory')
        before, _ = self.preserve_originals()
        self.output.write_bytes(b'optional report')
        original_open = os.open
        def deny_diagnostic(path, *args, **kwargs):
            if Path(path) == self.output:
                raise PermissionError('diagnostic read denied')
            return original_open(path, *args, **kwargs)
        with mock.patch.object(ci_apple.os, 'open', side_effect=deny_diagnostic):
            after, _ = self.preserve_originals()
        self.assertEqual(after, before)
        with mock.patch.object(apple_rust, 'CARGO_TIMING_BYTES', 1):
            after, _ = self.preserve_originals()
        self.assertEqual(after, before)
        self.output.unlink()
        self.output.symlink_to(self.root / 'mandatory.log')
        after, _ = self.preserve_originals()
        self.assertEqual(after, before)

    def test_optional_report_does_not_hide_mandatory_budget_or_read_failure(self):
        mandatory = self.root / 'mandatory.log'
        mandatory.write_bytes(b'mandatory')
        self.output.write_bytes(b'optional report')
        with mock.patch.object(ci_apple, 'EVIDENCE_ENTRIES', 0), \
                self.assertRaisesRegex(RuntimeError, 'selected original evidence entry budget exceeded'):
            self.preserve_originals()
        original_open = os.open
        def deny_mandatory(path, *args, **kwargs):
            if Path(path) == mandatory:
                raise PermissionError('mandatory read denied')
            return original_open(path, *args, **kwargs)
        with mock.patch.object(ci_apple.os, 'open', side_effect=deny_mandatory), \
                self.assertRaisesRegex(PermissionError, 'mandatory read denied'):
            self.preserve_originals()


if __name__ == '__main__':
    unittest.main()
