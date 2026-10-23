#!/usr/bin/env python3
"""Pure tooling receipt and launch contracts; no driver or product execution."""
import importlib.util
import io
import json
import os
from pathlib import Path
import tempfile
import sys
import unittest
from unittest.mock import Mock, patch

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('tooling', HERE / 'tooling.py')
T = importlib.util.module_from_spec(spec); spec.loader.exec_module(T)
spec = importlib.util.spec_from_file_location('package_linux', HERE / 'linux.py')
L = importlib.util.module_from_spec(spec); spec.loader.exec_module(L)


@unittest.skipUnless(sys.platform == 'linux', 'Linux package automation only')
class ToolingTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory(); self.addCleanup(self.tmp.cleanup)
        self.root = Path(self.tmp.name) / 'polaris-package-automation'
        (self.root / 'bin').mkdir(parents=True)
        self.driver = self.root / 'bin/tauri-driver'; self.driver.write_bytes(b'\x7fELFtest-only-not-executable-code'); self.driver.chmod(0o700)
        (self.root / '.crates.toml').write_text('[v1]\n"' + T.PACKAGE + '" = ["tauri-driver"]\n')
        self.env = {'GITHUB_ACTIONS': 'true', 'RUNNER_OS': 'Linux', 'GITHUB_REPOSITORY': 'polaris-arch/Polaris',
                    'GITHUB_SHA': 'a' * 40, 'GITHUB_RUN_ID': '120', 'GITHUB_RUN_ATTEMPT': '1',
                    'GITHUB_JOB': 'linux_tun_package_acceptance', 'RUNNER_NAME': 'runner-123',
                    'GITHUB_WORKFLOW_REF': 'polaris-arch/Polaris/.github/workflows/package.yml@refs/heads/test',
                    'RUNNER_TEMP': self.tmp.name}
        # Synthetic native bytes and root ownership; never execute either tool.
        self.native = Path(self.tmp.name) / 'WebKitWebDriver'
        self.native.write_bytes(b'\x7fELFnative-fixture'); self.native.chmod(0o755)
        original_stat = Path.stat
        native = self.native
        def mocked_stat(path, *args, **kwargs):
            value = original_stat(path, *args, **kwargs)
            if path == native:
                fields = list(value); fields[4] = 0; value = os.stat_result(fields)
            return value
        stat_patch = patch.object(Path, 'stat', mocked_stat); stat_patch.start(); self.addCleanup(stat_patch.stop)
        self.patch = patch.object(T, 'NATIVE', self.native); self.patch.start(); self.addCleanup(self.patch.stop)
        self.run = Mock(side_effect=lambda argv: f'webkit2gtk-driver: {self.native}\n' if '-S' in argv
                        else 'webkit2gtk-driver\t2.50.0-0ubuntu0.22.04.1\tamd64\tinstalled\n')
        self.receipt = self.root / 'receipt.json'

    def prepare(self):
        return T.prepare(self.root, self.receipt, self.env, self.run)

    def test_exact_inventory_and_actual_bytes_not_claimed_version_stdout(self):
        facts = self.prepare()
        self.assertEqual(T.verify(self.env, self.run), facts)
        self.assertEqual(facts['automation']['version'], '2.0.5')
        self.assertEqual(facts['automation']['binary']['sha256'], T.identity(self.driver)['sha256'])
        self.assertEqual(facts['nativeDriver']['version'], '2.50.0-0ubuntu0.22.04.1')
        self.assertEqual(self.receipt.stat().st_mode & 0o777, 0o600)
        self.assertTrue(all(call.args[0][0] == '/usr/bin/dpkg-query' for call in self.run.call_args_list))
        with self.assertRaises(FileExistsError): self.prepare()

    def test_wrong_version_registry_binary_and_multiple_packages_rejected(self):
        good = (self.root / '.crates.toml').read_bytes()
        for raw in [good.replace(b'2.0.5', b'2.0.6'), good.replace(b'registry+', b'git+'),
                    good.replace(b'github.com/rust-lang/crates.io-index', b'example.com/index'),
                    good.replace(b'["tauri-driver"]', b'["other"]'), good+b'"extra" = ["other"]\n',
                    b'', good+b'# arbitrary inventory suffix']:
            with self.subTest(raw=raw), self.assertRaises(ValueError): T.cargo_package(raw)

    def test_context_or_binary_drift_and_missing_receipt_fail(self):
        with self.assertRaises(ValueError): T.verify(self.env, self.run)
        self.prepare()
        for key, value in [('GITHUB_SHA', 'b'*40), ('GITHUB_RUN_ID', '121'), ('GITHUB_RUN_ATTEMPT', '2'),
                           ('GITHUB_JOB', 'other'), ('RUNNER_NAME', 'other'), ('GITHUB_WORKFLOW_REF', 'other')]:
            with self.subTest(key=key), self.assertRaises(ValueError): T.verify({**self.env, key: value}, self.run)
        self.driver.write_bytes(b'\x7fELFchanged')
        with self.assertRaises(ValueError): T.verify(self.env, self.run)

    def test_foreign_native_package_status_arch_or_observation_failure_rejected(self):
        for output in ['other\t2.50.0\tamd64\tinstalled', 'webkit2gtk-driver\t2.50.0\tarm64\tinstalled',
                       'webkit2gtk-driver\t2.50.0\tamd64\tconfig-files', 'webkit2gtk-driver\t\tamd64\tinstalled']:
            run = lambda argv: f'webkit2gtk-driver: {self.native}' if '-S' in argv else output
            with self.subTest(output=output), self.assertRaises(ValueError): T.observe(self.root, self.env, run)
        with self.assertRaises(ValueError): T.observe(self.root, self.env, lambda argv: 'foreign owner')
        with self.assertRaises(OSError): T.observe(self.root, self.env, Mock(side_effect=OSError('dpkg unavailable')))

    def test_receipt_permissions_links_and_nonelf_fail_closed(self):
        self.prepare(); self.receipt.chmod(0o644)
        with self.assertRaises(ValueError): T.verify(self.env, self.run)
        self.receipt.chmod(0o600); self.driver.write_bytes(b'#!/bin/sh\necho tauri-driver 2.0.5\n')
        with self.assertRaises(ValueError): T.verify(self.env, self.run)
        self.driver.unlink(); self.driver.symlink_to(self.native)
        with self.assertRaises(ValueError): T.verify(self.env, self.run)

    def test_native_hash_drift_cannot_be_relabelled_as_same_package(self):
        self.prepare(); original = T.identity
        def changed(path, executable=False):
            value = original(path, executable)
            if Path(path) == self.native: value['sha256'] = '0'*64
            return value
        with patch.object(T, 'identity', side_effect=changed), self.assertRaises(ValueError): T.verify(self.env, self.run)

    def test_shared_actual_start_tools_uses_proved_paths_and_supported_args(self):
        facts = self.prepare(); obj = object.__new__(L.CarrierRun)
        obj.env = {'PATH': '/usr/bin:/bin'}; obj.result = {}; obj.supervisor = Mock()
        bus = Mock(); bus.stdout = io.BytesIO(b'unix:path=/tmp/mock-private-bus\n')
        obj.supervisor.spawn.side_effect = [Mock(), bus, Mock()]
        web = Mock(); web.request.return_value = {'ready': True}
        with patch.object(L, 'verified_tools', return_value=facts), patch.object(L.os, 'read', return_value=b'99\n'), \
             patch('select.select', return_value=([1], [], [])), patch.object(L, 'free_port', side_effect=[14444,14445]), \
             patch.object(L, 'WebDriver', return_value=web):
            obj.start_tools()
        commands = [c.args[0] for c in obj.supervisor.spawn.call_args_list]
        self.assertEqual(commands[-1], [str(self.driver), '--port', '14444', '--native-port', '14445', '--native-driver', str(self.native)])
        self.assertFalse(any('--version' in command or '--help' in command for command in commands))
        self.assertEqual(obj.result['tools']['identity'], facts)
        self.assertFalse(any(k.startswith(('GITHUB_', 'RUNNER_')) for k in obj.env))


if __name__ == '__main__': unittest.main()
