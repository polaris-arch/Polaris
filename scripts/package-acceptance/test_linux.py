#!/usr/bin/env python3
"""Source/unit tests only: no product, WebKit, installed package or privilege."""
import importlib.util
import json
import os
from pathlib import Path
import socket
import struct
import sys
import tempfile
import unittest
from unittest.mock import patch, Mock

SPEC = importlib.util.spec_from_file_location('package_linux', Path(__file__).with_name('linux.py'))
M = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(M)
SOURCE = Path(os.environ.get('POLARIS_ACCEPTANCE_TEST_SOURCE_ROOT', M.ROOT))


@unittest.skipUnless(sys.platform == 'linux', 'Linux-only /proc, uid, pidfd and POSIX package supervisor')
class PrimitiveTests(unittest.TestCase):
    def test_fresh_clean_exit_marker_required(self):
        with tempfile.TemporaryDirectory() as tmp:
            marker = Path(tmp) / 'clean-exit.marker'
            started = M.time.time_ns()
            with self.assertRaises(ValueError):
                M.clean_exit_marker(marker, started)
            marker.write_bytes(b'')
            self.assertTrue(M.clean_exit_marker(marker, started)['createdAfterShortcut'])
            with self.assertRaises(ValueError):
                M.clean_exit_marker(marker, marker.stat().st_mtime_ns + 2_000_000_000)
            marker.write_bytes(b'not-a-clean-commit')
            with self.assertRaises(ValueError):
                M.clean_exit_marker(marker, started)

    def test_runtime_original_bytes_are_private_not_reconstructed(self):
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp)
            runtime = home / 'singbox-runtime.json'
            original = b'{ "services": [{"type":"api","secret":"private","dashboard":{"enabled":true,"http_client":{"detour":"proxy"}}}], "route":{"final":"proxy"}}\n'
            runtime.write_bytes(original)
            identity = {'pid': 777, 'startTime': '12'}
            with patch.object(M, 'process_identity', return_value=identity):
                result = M.stable_runtime_capture(runtime, home, identity, home / 'originals')
            self.assertEqual(result['sha256'], M.hashlib.sha256(original).hexdigest())
            self.assertNotIn('private', json.dumps(result).replace('private ephemeral', 'ephemeral'))
            archive = next((home / 'originals').iterdir())
            self.assertEqual(archive.stat().st_mode & 0o777, 0o600)
            self.assertEqual(M.base64.b64decode(json.loads(archive.read_text())['originalBytesBase64']), original)
            self.assertTrue(result['dashboardEnabled'])
            with patch.object(M, 'process_identity', return_value=None), self.assertRaises(ValueError):
                M.stable_runtime_capture(runtime, home, identity, home / 'originals')
            runtime.write_bytes(original.replace(b'"enabled":true', b'"enabled":false'))
            with patch.object(M, 'process_identity', return_value=identity), self.assertRaises(ValueError):
                M.stable_runtime_capture(runtime, home, identity, home / 'originals')

    def test_cross_language_edit_version(self):
        fixture = json.loads((SOURCE / 'ui/src/contracts/config-version.fixture.json').read_text())
        for row in fixture['cases']:
            with self.subTest(row['name']):
                self.assertEqual(M.config_version(row['config']), row['expected'])

    def test_identity_rejects_pid_reuse(self):
        first = {'pid': 7, 'startTime': '123'}
        self.assertTrue(M.same_process(first, dict(first)))
        self.assertFalse(M.same_process(first, {'pid': 7, 'startTime': '124'}))
        self.assertFalse(M.same_process(first, None))

    def test_actual_proc_parse(self):
        identity = M.process_identity(os.getpid())
        self.assertEqual(identity['pid'], os.getpid())
        self.assertEqual(identity['uid'], os.getuid())
        self.assertEqual(identity['ppid'], os.getppid())
        self.assertTrue(identity['startTime'].isdigit())

    def test_regular_rejects_links_and_outside(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            file = root / 'plain'
            file.write_bytes(b'x')
            self.assertEqual(M.regular(file, root), file)
            link = root / 'link'
            link.symlink_to(file)
            with self.assertRaises(ValueError):
                M.regular(link, root)
            with self.assertRaises(ValueError):
                M.regular(file, root / 'elsewhere')

    def test_squashfs_unique_and_bound(self):
        with tempfile.TemporaryDirectory() as tmp:
            file = Path(tmp) / 'AppImage'
            block = bytearray(96)
            block[:4] = b'hsqs'
            struct.pack_into('<I', block, 4, 1)
            struct.pack_into('<I', block, 12, 65536)
            struct.pack_into('<HH', block, 28, 4, 0)
            struct.pack_into('<Q', block, 40, 96)
            file.write_bytes(b'ELF-prefix' + block)
            self.assertEqual(M.squashfs_offset(file), len(b'ELF-prefix'))
            file.write_bytes(block + block)
            with self.assertRaises(ValueError):
                M.squashfs_offset(file)
            file.write_bytes(b'hsqs')
            with self.assertRaises(ValueError):
                M.squashfs_offset(file)

    def test_exact_carrier_hash_missing_duplicate_and_changed(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            file = root / 'Polaris.deb'
            file.write_bytes(b'final')
            item = {'name': file.name, 'bytes': 5, 'sha256': M.sha(file)}
            self.assertEqual(M.carrier_path(root, item), file)
            file.write_bytes(b'other')
            with self.assertRaises(ValueError):
                M.carrier_path(root, item)
            file.write_bytes(b'final')
            (root / 'extra').mkdir()
            (root / 'extra' / file.name).write_bytes(b'final')
            with self.assertRaises(ValueError):
                M.carrier_path(root, item)

    def test_empty_home_environment_allowlist(self):
        with tempfile.TemporaryDirectory() as tmp, patch.dict(os.environ, {
            'GH_TOKEN': 'private', 'GITHUB_TOKEN': 'private', 'HTTPS_PROXY': 'https://proxy.invalid',
            'LD_PRELOAD': '/other', 'HOME': '/unrelated', 'PATH': '/unrelated'}, clear=True), patch.object(M.os, 'geteuid', return_value=1000):
            env = M.private_env(Path(tmp))
            self.assertEqual(env['HOME'], tmp)
            self.assertEqual(env['TAURI_WEBVIEW_AUTOMATION'], 'true')
            self.assertNotIn('GH_TOKEN', env)
            self.assertNotIn('HTTPS_PROXY', env)
            self.assertNotIn('LD_PRELOAD', env)
            self.assertNotIn('GITHUB_TOKEN', env)
            self.assertNotIn('DISPLAY', env)
            for name in ('XDG_CONFIG_HOME', 'XDG_DATA_HOME', 'XDG_CACHE_HOME', 'XDG_STATE_HOME', 'XDG_RUNTIME_DIR'):
                self.assertEqual(Path(env[name]).stat().st_mode & 0o777, 0o700)

    def test_default_sandbox_refuses_all_overrides_and_root(self):
        with tempfile.TemporaryDirectory() as tmp:
            for key in M.FORBIDDEN:
                with self.subTest(key), patch.dict(os.environ, {key: '0'}, clear=True), patch.object(M.os, 'geteuid', return_value=1000):
                    with self.assertRaises(ValueError):
                        M.private_env(Path(tmp))
            with patch.dict(os.environ, {}, clear=True), patch.object(M.os, 'geteuid', return_value=0):
                with self.assertRaises(ValueError):
                    M.private_env(Path(tmp))

    def test_ipv4_socket_observation(self):
        row = M.parse_proc_socket(' 0: 0100007F:9C40 08080808:01BB 01 0:0 00:0 0 1000 0 998 1')
        self.assertEqual(row['remoteIp'], '8.8.8.8')
        self.assertEqual(row['remotePort'], 443)
        self.assertEqual(row['inode'], '998')
        self.assertFalse(row['loopback'])
        loop = M.parse_proc_socket(' 0: 0:0 0100007F:0050 01 0:0 00:0 0 1000 0 998 1')
        self.assertTrue(loop['loopback'])

    def test_ipv6_socket_observation(self):
        row = M.parse_proc_socket(' 0: 0:0 00000000000000000000000001000000:01BB 01 0:0 00:0 0 1000 0 999 1', True)
        self.assertEqual(row['remoteIp'], '::1')
        self.assertTrue(row['loopback'])
        self.assertIsNone(M.parse_proc_socket('malformed'))
        self.assertIsNone(M.parse_proc_socket(' 0: 0:0 zz:GG 01 0:0 00:0 0 1000 0 999 1'))

    def test_unknown_apply_cannot_resubmit(self):
        run = M.CarrierRun.__new__(M.CarrierRun)
        run.driver = Mock()
        run.driver.observations.side_effect = [[], [{'source': 'original-native-callback', 'sequence': 1,
                                                   'success': False, 'outcome': 'unknown'}]]
        run.text = lambda key: key
        with self.assertRaises(ValueError):
            run.apply()
        run.driver.click.assert_called_once()
        run.driver.invoke.assert_not_called()

    def test_unconfirmed_apply_only_reads_same_transaction(self):
        run = M.CarrierRun.__new__(M.CarrierRun)
        run.driver = Mock()
        run.driver.observations.side_effect = [[], [{'source': 'original-native-callback', 'sequence': 1,
                                                   'success': True, 'status': 'unconfirmed', 'transactionId': 'original'}]]
        run.driver.invoke.return_value = {'status': 'hotApplied', 'transactionId': 'original'}
        run.text = lambda key: key
        self.assertEqual(run.apply()['transactionId'], 'original')
        run.driver.click.assert_called_once()
        run.driver.invoke.assert_called_once_with('proxy_runtime_apply_status', {'transactionId': 'original'})

    def test_wrong_retry_transaction_fails(self):
        run = M.CarrierRun.__new__(M.CarrierRun)
        run.driver = Mock()
        run.driver.observations.side_effect = [[], [{'source': 'original-native-callback', 'sequence': 1,
                                                   'success': True, 'status': 'hotApplied', 'transactionId': 'other'}]]
        run.text = lambda key: key
        with patch.object(M, 'wait_for', side_effect=lambda read, **kwargs: read()), self.assertRaises((ValueError, AttributeError)):
            run.apply(retry='original')

    def test_pidfd_prevents_signal_pid_reuse_window(self):
        supervisor = M.Supervisor.__new__(M.Supervisor)
        item = {'pid': 9, 'startTime': 'old'}
        supervisor.owned = {9: item}
        with patch.object(M, 'process_identity', return_value=item), patch.object(M.os, 'pidfd_open', return_value=27) as opened, patch.object(M.signal, 'pidfd_send_signal') as sent, patch.object(M.os, 'close') as closed:
            supervisor.signal(item, 15)
            opened.assert_called_once_with(9, 0)
            sent.assert_called_once_with(27, 15, None, 0)
            closed.assert_called_once_with(27)
        with patch.object(M, 'process_identity', side_effect=[item, {'pid': 9, 'startTime': 'new'}]), patch.object(M.os, 'pidfd_open', return_value=28), patch.object(M.signal, 'pidfd_send_signal') as sent, patch.object(M.os, 'close') as closed:
            with self.assertRaises(ValueError):
                supervisor.signal(item, 15)
            sent.assert_not_called()
            closed.assert_called_once_with(28)

    def test_signal_requires_owned_starttime_and_lineage(self):
        supervisor = M.Supervisor.__new__(M.Supervisor)
        item = {'pid': 9, 'startTime': 'old'}
        supervisor.owned = {9: item}
        with patch.object(M, 'process_identity', return_value={'pid': 9, 'startTime': 'new'}), patch.object(M.os, 'kill') as kill:
            with self.assertRaises(ValueError):
                supervisor.signal(item, 15)
            kill.assert_not_called()
        supervisor.owned = {}
        with patch.object(M, 'process_identity', return_value=item), patch.object(M.os, 'kill') as kill:
            with self.assertRaises(ValueError):
                supervisor.signal(item, 15)
            kill.assert_not_called()

    def test_click_uses_webdriver_input_not_synthetic_events(self):
        driver = M.WebDriver(4444)
        driver.session = 'native'
        driver.element = Mock(return_value={M.ELEMENT: 'actual-element'})
        driver.request = Mock()
        driver.click('#connect-btn')
        driver.request.assert_called_once_with('POST', '/session/native/element/actual-element/click', {})

    def test_observer_is_passthrough_and_bounded(self):
        self.assertIn('Reflect.apply(callback, this, args)', M.OBSERVER)
        self.assertIn('internals.invoke !== originalInvoke', M.OBSERVER)
        self.assertIn('observation.rows.length > 512', M.OBSERVER)
        self.assertNotIn('config_save', M.OBSERVER)
        self.assertNotIn('secret', M.OBSERVER)
        self.assertNotIn('args[1] =', M.OBSERVER)

    def test_native_rejection_is_not_unknown(self):
        driver = M.WebDriver(4444)
        driver.session = 'native'
        driver.request = Mock(return_value={'error': 'native-command-rejected', 'rejected': True})
        with self.assertRaises(M.NativeRejected):
            driver.invoke('config_save', {})
        driver.request.return_value = {'error': 'native-command-failed'}
        with self.assertRaises(ValueError) as caught:
            driver.invoke('config_get')
        self.assertNotIsInstance(caught.exception, M.NativeRejected)

    def test_mutating_ipc_cannot_replace_gui(self):
        driver = M.WebDriver(4444)
        for command in ('proxy_start', 'proxy_stop', 'server_switch', 'auto_select_enable',
                        'proxy_apply_pending_changes', 'tray_quit'):
            with self.subTest(command), self.assertRaises(ValueError):
                driver.invoke(command)

    def test_cronet_maps_empty_is_not_loaded_claim(self):
        with tempfile.TemporaryDirectory() as tmp:
            library = Path(tmp) / 'libcronet.so'
            library.write_bytes(b'packaged')
            result = M.cronet_mapping('', library, M.sha(library))
            self.assertEqual(result['mapState'], 'not-mapped-in-socks-scenario')
            self.assertIsNone(result['mappedPath'])
            prefix = '0000-1111 r-xp 0000 00:00 1 '
            result = M.cronet_mapping(prefix + str(library), library, M.sha(library))
            self.assertEqual(result['mapState'], 'mapped-exact-packaged-library')
            for maps in (prefix + str(library) + ' (deleted)', prefix + '/foreign/libcronet.so',
                         prefix + str(library) + '\n' + prefix + '/foreign/libcronet.so'):
                with self.subTest(maps), self.assertRaises(ValueError):
                    M.cronet_mapping(maps, library, M.sha(library))
            with self.assertRaises(ValueError):
                M.cronet_mapping('', library, '0' * 64)

    def test_leftover_product_never_becomes_normal_after_cleanup(self):
        supervisor = M.Supervisor.__new__(M.Supervisor)
        product = {'pid': 5, 'startTime': '9'}
        supervisor.living = Mock(side_effect=[[product], [product], [], [], []])
        supervisor.signal = Mock()
        supervisor.children = {}
        supervisor.forced = []
        supervisor.unknown = []
        supervisor.stop = Mock()
        supervisor.thread = Mock()
        with patch.object(M.os, 'waitpid', side_effect=ChildProcessError):
            result = supervisor.close(True)
        self.assertEqual(result['state'], 'failed')
        self.assertEqual(result['remaining'], [])
        self.assertEqual(result['forced'][0]['reason'], 'product-not-exited')

    def test_unknown_cleanup_never_passes(self):
        supervisor = M.Supervisor.__new__(M.Supervisor)
        supervisor.living = Mock(return_value=[])
        supervisor.children = {}
        supervisor.forced = []
        supervisor.unknown = ['unobserved-adopted-descendant']
        supervisor.stop = Mock()
        supervisor.thread = Mock()
        with patch.object(M.os, 'waitpid', side_effect=ChildProcessError):
            result = supervisor.close(True)
        self.assertEqual(result['state'], 'failed')
        self.assertEqual(result['unknown'], ['unobserved-adopted-descendant'])

    def test_cancel_is_failure_not_skip(self):
        with self.assertRaises(M.AcceptanceCancelled):
            M.cancellation(15, None)

    def test_all_mandatory_cases_and_explicit_scope(self):
        self.assertEqual(len(M.CASES), 12)
        source = Path(M.__file__).read_text()
        self.assertIn("invalid_result = self.apply('rejected')", source)
        self.assertIn("'candidateSetup': 'native-config-save-fixture'", source)
        self.assertIn("'zeroEgressClaim': False", source)
        self.assertIn('ProxyHandler({})', source)
        self.assertNotIn("except NativeRejected:", source)
        self.assertIn("self.restart_case(wanted)", source)
        self.assertLess(source.index('self.restart_case(wanted)'), source.index("rpc = self.rpc('suite'"))

    def test_byte_inspection_keeps_original_entry_and_layout(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            carrier = root / 'final.AppImage'
            carrier.write_bytes(b'placeholder')
            destination = root / 'payload'
            def fake_extract(*args, **kwargs):
                (destination / 'usr/bin').mkdir(parents=True)
                resources = destination / 'usr/lib/Polaris/_up_/resources/linux'
                resources.mkdir(parents=True)
                for path, data in [(destination / 'usr/bin/polaris', b'\x7fELFapp'),
                                   (resources / 'sing-box', b'\x7fELFcore'),
                                   (resources / 'libcronet.so', b'\x7fELFlibrary'),
                                   (destination / 'AppRun', b'#!/bin/sh\noriginal-hooks\n')]:
                    path.write_bytes(data)
                    path.chmod(0o755)
                (destination / 'apprun-hooks').mkdir()
                (destination / 'apprun-hooks/linuxdeploy-plugin-gtk.sh').write_text('original-hook')
            with patch.object(M, 'squashfs_offset', return_value=10), patch.object(M.subprocess, 'run', side_effect=fake_extract), patch.object(M.os, 'geteuid', return_value=1000):
                members, entry = M.extract(carrier, 'AppImage', destination)
            self.assertEqual(entry, destination / 'AppRun')
            self.assertEqual(set(members), {'app', 'core', 'cronet'})
            self.assertEqual(members['app']['path'], 'usr/bin/polaris')
            self.assertEqual(entry.read_text(), '#!/bin/sh\noriginal-hooks\n')
            self.assertEqual((destination / 'apprun-hooks/linuxdeploy-plugin-gtk.sh').read_text(), 'original-hook')


if __name__ == '__main__':
    unittest.main()
