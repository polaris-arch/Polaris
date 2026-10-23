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
from types import SimpleNamespace
from unittest.mock import patch, Mock, mock_open

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
        item = {'pid': 9, 'startTime': 'old', 'uid': os.getuid()}
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
        item = {'pid': 9, 'startTime': 'old', 'uid': os.getuid()}
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

    def test_same_birth_uid_change_blocks_pidfd_signal(self):
        supervisor = M.Supervisor.__new__(M.Supervisor)
        item = {'pid': 9, 'startTime': 'old', 'uid': os.getuid()}
        elevated = {**item, 'uid': 0 if os.getuid() else 1}
        supervisor.owned = {9: item}
        with patch.object(M, 'process_identity', side_effect=[item, elevated]), patch.object(M.os, 'pidfd_open', return_value=28), patch.object(M.signal, 'pidfd_send_signal') as sent, patch.object(M.os, 'close') as closed:
            with self.assertRaises(ValueError): supervisor.signal(item, 15)
            sent.assert_not_called(); closed.assert_called_once_with(28)

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
        supervisor.thread.is_alive.return_value = False
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
        supervisor.thread.is_alive.return_value = False
        with patch.object(M.os, 'waitpid', side_effect=ChildProcessError):
            result = supervisor.close(True)
        self.assertEqual(result['state'], 'failed')
        self.assertEqual(result['unknown'], ['unobserved-adopted-descendant'])

    def test_close_inventory_exceptions_always_stop_monitor(self):
        for failure_at in range(5):
            supervisor = M.Supervisor.__new__(M.Supervisor)
            supervisor.living = Mock(side_effect=[[]] * failure_at + [ValueError('private text must not leak')])
            supervisor.signal = Mock()
            supervisor.children = {}
            supervisor.forced = []
            supervisor.unknown = []
            supervisor.stop = Mock()
            supervisor.thread = Mock()
            supervisor.thread.is_alive.return_value = False
            with patch.object(M.os, 'waitpid', side_effect=ChildProcessError):
                result = supervisor.close(True)
            supervisor.stop.set.assert_called_once()
            supervisor.thread.join.assert_called_once_with(2)
            supervisor.signal.assert_not_called()
            self.assertEqual(result['state'], 'failed')
            self.assertEqual(result['remainingAssessment'], 'unknown')
            self.assertNotIn('private text', json.dumps(result))

    def test_close_monitor_that_did_not_stop_fails(self):
        supervisor = M.Supervisor.__new__(M.Supervisor)
        supervisor._close_owned = Mock(return_value={'state': 'normal', 'forced': [], 'remaining': [], 'unknown': []})
        supervisor.forced, supervisor.unknown = [], []
        supervisor.stop, supervisor.thread = Mock(), Mock()
        supervisor.thread.is_alive.return_value = True
        result = supervisor.close(True)
        self.assertEqual(result['state'], 'failed')
        self.assertIn('monitor-stop-unconfirmed', result['unknown'])
        self.assertEqual(result['remainingAssessment'], 'unknown')
        self.assertIn('threading.Thread(target=self._monitor, daemon=True)', Path(M.__file__).read_text())

    def test_identity_diagnostic_is_bounded_and_not_authorization(self):
        identity = {'pid': 17, 'startTime': '25', 'ppid': 9, 'uid': 0}
        status = 'Uid: 1000 1000 1000 1000\nGid: 1000 1000 1000 1000\nCapEff: 0\nCapPrm: 0\nCapAmb: 0\nNoNewPrivs: 1\nSeccomp: 2\nName: safe-name\nIgnored: secret-argv\n'
        files = [mock_open(read_data=status).return_value, mock_open(read_data='0 1000 1\n').return_value]
        with patch.object(M, 'process_identity', return_value=identity), patch.object(M.Path, 'open', side_effect=files), patch.object(M.os, 'readlink', side_effect=['user:[3]', 'mnt:[4]', 'pid:[5]']), patch.object(M, 'executable_diagnostic', return_value={'state': 'unknown', 'errno': 13}):
            result = M.process_diagnostic(identity)
        self.assertEqual(result['Uid'], ['1000'] * 4)
        self.assertEqual(result['uid'], 0)  # Original rejection UID remains unchanged.
        self.assertEqual(result['uidMap'], [['0', '1000', '1']])
        self.assertNotIn('secret-argv', json.dumps(result))
        self.assertIn('not-authorization', result['observation'])
        with patch.object(M, 'process_identity', return_value=identity), patch.object(M.Path, 'open', side_effect=PermissionError):
            self.assertEqual(M.process_diagnostic(identity)['observation'], 'unknown')
        with patch.object(M, 'process_identity', return_value={**identity, 'startTime': '26'}):
            self.assertEqual(M.process_diagnostic(identity)['observation'], 'unknown')

    def diagnostic_supervisor(self):
        obj = M.Supervisor.__new__(M.Supervisor)
        obj.lock = M.threading.RLock()
        obj.owned, obj.roles, obj.children = {}, {}, {}
        obj.diagnostics, obj.unknown = [], []
        obj.diagnostics_truncated = False
        obj.diagnostic_executable_budget = [M.DIAGNOSTIC_EXE_LIMIT]
        return obj

    def test_diagnostic_role_chain_stops_at_known_direct_spawn(self):
        obj = self.diagnostic_supervisor()
        tool = {'pid': 10, 'startTime': '100', 'ppid': 1, 'uid': os.getuid()}
        parent = {'pid': 11, 'startTime': '101', 'ppid': 10, 'uid': os.getuid()}
        foreign = {'pid': 12, 'startTime': '102', 'ppid': 11, 'uid': 0}
        obj.owned = {10: tool, 11: parent}; obj.roles = {10: 'dbus', 11: 'dbus'}; obj.children = {10: Mock()}
        with patch.object(M, 'process_identity', side_effect=lambda pid: obj.owned.get(pid)), patch.object(M, 'process_diagnostic', side_effect=lambda item, **kw: {**item, 'observation': 'same-birth-snapshot-not-authorization'}):
            obj.record_diagnostic('owned-descendant-uid-mismatch', foreign, parent)
            obj.record_diagnostic('owned-descendant-uid-mismatch', foreign, parent)
        self.assertEqual(len(obj.diagnostics), 1)
        row = obj.diagnostics[0]
        self.assertEqual(row['parentRole'], 'dbus')
        self.assertEqual(row['process']['directSpawnRole'], 'unknown')
        self.assertEqual(row['ancestors'][-1]['directSpawnRole'], 'dbus')
        self.assertEqual(row['ancestryState'], 'direct-spawn-reached')
        self.assertNotIn(12, obj.owned)  # Observation does not adopt the birth.

    def test_diagnostic_ancestry_cycle_depth_and_pid_reuse_fail_closed(self):
        for fault in ('cycle', 'depth-limit', 'unconfirmed-birth'):
            obj = self.diagnostic_supervisor()
            obj.owned = {pid: {'pid': pid, 'startTime': str(pid), 'ppid': pid + 1, 'uid': os.getuid()} for pid in range(10, 21)}
            if fault == 'cycle': obj.owned[11]['ppid'] = 10
            item = {'pid': 9, 'startTime': '9', 'ppid': 10, 'uid': 0}
            def current(pid):
                value = obj.owned.get(pid)
                return {**value, 'startTime': 'reused'} if fault == 'unconfirmed-birth' else value
            with patch.object(M, 'process_identity', side_effect=current), patch.object(M, 'process_diagnostic', side_effect=lambda item, **kw: {**item, 'observation': 'same-birth-snapshot-not-authorization'}):
                obj.record_diagnostic('foreign-process', item)
            row = obj.diagnostics[0]
            self.assertEqual(row['ancestryState'], fault)
            self.assertLessEqual(len(row['ancestors']), 8)
            if fault == 'unconfirmed-birth':
                self.assertEqual(row['parentRole'], 'unknown')
                self.assertEqual(row['parent']['pid'], 10)
                self.assertEqual(row['parent']['startTime'], '10')
                self.assertEqual(row['parent']['role'], 'unknown')
                self.assertEqual(row['parent']['observation'], 'inventory-birth-not-revalidated')

    def test_diagnostic_failures_do_not_replace_original_uid_rejection(self):
        obj = self.diagnostic_supervisor()
        parent = {'pid': 10, 'startTime': '10', 'ppid': 1, 'uid': os.getuid()}
        foreign = {'pid': 11, 'startTime': '11', 'ppid': 10, 'uid': 1 if os.getuid() == 0 else 0}
        obj.owned = {10: parent}; obj.roles = {10: 'driver'}; obj.children = {10: Mock()}
        with patch.object(M.Path, 'iterdir', return_value=[Path('/proc/10'), Path('/proc/11')]), patch.object(M, 'process_identity', side_effect=lambda pid: {10: parent, 11: foreign}[pid]), patch.object(M, 'process_diagnostic', side_effect=PermissionError(13, 'private-error')):
            with self.assertRaisesRegex(ValueError, 'owned descendant elevated unexpectedly'):
                obj.scan()
        self.assertNotIn(11, obj.owned)
        self.assertIn('identity-diagnostic-unavailable', obj.unknown)
        self.assertNotIn('private-error', json.dumps(obj.unknown))

    def test_executable_diagnostic_stable_regular_bytes_and_budget(self):
        item = {'pid': 17, 'startTime': '25'}
        raw = b'not-an-executable-fixture'
        info = SimpleNamespace(st_mode=M.stat.S_IFREG | 0o4755, st_dev=2, st_ino=4, st_size=len(raw), st_uid=0, st_gid=0, st_mtime_ns=6, st_ctime_ns=7)
        budget = [len(raw)]
        with patch.object(M, 'process_identity', return_value=item), patch.object(M.os, 'readlink', return_value='/usr/bin/fixture'), patch.object(M.Path, 'open', mock_open(read_data=raw)), patch.object(M.os, 'fstat', return_value=info), patch.object(M.Path, 'stat', return_value=info):
            result = M.executable_diagnostic(item, budget)
            blocked = M.executable_diagnostic(item, budget)
        self.assertEqual(result['state'], 'observed-not-authorization')
        self.assertEqual(result['sha256'], M.hashlib.sha256(raw).hexdigest())
        self.assertEqual(result['mode'], 0o4755)
        self.assertEqual(budget, [0])
        self.assertEqual(blocked['state'], 'unknown')
        self.assertEqual(blocked['failureStage'], 'executable-byte-budget')
        self.assertNotIn('sha256', blocked)

    def test_executable_diagnostic_unreadable_reused_changed_and_deadline(self):
        item = {'pid': 17, 'startTime': '25'}
        with patch.object(M, 'process_identity', return_value=item), patch.object(M.os, 'readlink', side_effect=PermissionError(13, 'secret')):
            denied = M.executable_diagnostic(item, [100])
        self.assertEqual(denied['errno'], 13)
        self.assertNotIn('secret', json.dumps(denied))
        with patch.object(M, 'process_identity', return_value={**item, 'startTime': '26'}):
            self.assertEqual(M.executable_diagnostic(item, [100])['state'], 'unstable')
        info = SimpleNamespace(st_mode=M.stat.S_IFREG | 0o755, st_dev=2, st_ino=4, st_size=3, st_uid=0, st_gid=0, st_mtime_ns=6, st_ctime_ns=7)
        changed = SimpleNamespace(**{**vars(info), 'st_ino': 8})
        for fault in ('exec-change', 'oversize', 'deadline', 'invalid-link'):
            with self.subTest(fault=fault), patch.object(M, 'process_identity', return_value=item), patch.object(M.os, 'readlink', return_value='bad\nlink' if fault == 'invalid-link' else '/usr/bin/fixture'), patch.object(M.Path, 'open', mock_open(read_data=b'abc')), patch.object(M.os, 'fstat', return_value=info), patch.object(M.Path, 'stat', return_value=changed), patch.object(M.time, 'monotonic', side_effect=[0, 2] if fault == 'deadline' else None, return_value=0):
                result = M.executable_diagnostic(item, [2 if fault == 'oversize' else 100])
                self.assertEqual(result['state'], 'unknown')
                self.assertNotIn('sha256', result)

    def test_executable_diagnostic_pid_reuse_after_read_discards_facts(self):
        item = {'pid': 17, 'startTime': '25'}
        info = SimpleNamespace(st_mode=M.stat.S_IFREG | 0o755, st_dev=2, st_ino=4, st_size=3, st_uid=0, st_gid=0, st_mtime_ns=6, st_ctime_ns=7)
        with patch.object(M, 'process_identity', side_effect=[item, {**item, 'startTime': '26'}]), patch.object(M.os, 'readlink', return_value='/usr/bin/fixture'), patch.object(M.Path, 'open', mock_open(read_data=b'abc')), patch.object(M.os, 'fstat', return_value=info), patch.object(M.Path, 'stat', return_value=info):
            result = M.executable_diagnostic(item, [100])
        self.assertEqual(result, {'state': 'unstable'})

    def test_name_hint_invalid_characters_are_unknown(self):
        identity = {'pid': 17, 'startTime': '25', 'ppid': 9, 'uid': 0}
        for name in ('unsafe\x1bname', 'x' * 65, ''):
            files = [mock_open(read_data='Name: ' + name + '\nUid: 1 0 0 0\n').return_value,
                     mock_open(read_data='0 0 1\n').return_value]
            with patch.object(M, 'process_identity', return_value=identity), patch.object(M.Path, 'open', side_effect=files), patch.object(M.os, 'readlink', side_effect=PermissionError(13, 'private')), patch.object(M, 'executable_diagnostic', return_value={'state': 'unknown'}):
                result = M.process_diagnostic(identity)
            self.assertEqual(result['nameHint'], 'unknown')
            self.assertNotIn('private', json.dumps(result))
            self.assertEqual(result['userNamespaceError']['errno'], 13)

    def test_diagnostic_supervisor_cumulative_byte_budget_and_record_cap(self):
        obj = self.diagnostic_supervisor()
        def snapshot(item, *, executable_budget):
            self.assertIs(executable_budget, obj.diagnostic_executable_budget)
            executable_budget[0] -= 1
            return dict(item)
        with patch.object(M, 'process_diagnostic', side_effect=snapshot):
            for pid in range(65):
                obj.record_diagnostic('foreign-process', {'pid': pid, 'startTime': str(pid), 'ppid': -1, 'uid': 0})
        self.assertEqual(len(obj.diagnostics), 64)
        self.assertTrue(obj.diagnostics_truncated)
        self.assertEqual(obj.diagnostic_executable_budget[0], M.DIAGNOSTIC_EXE_LIMIT - 64)

    def test_workspace_mount_and_permission_failures_are_not_bypassed(self):
        mount = '35 20 0:32 / /tmp/owned/home/runtime/doc rw - fuse.portal portal rw\n'
        with patch.object(M.Path, 'read_text', return_value=mount), patch.object(M.shutil, 'rmtree') as remove:
            result = M.cleanup_workspace('/tmp/owned', processes_stopped=True)
        self.assertEqual(result['reason'], 'workspace-mount-remains')
        remove.assert_not_called()
        with patch.object(M.Path, 'read_text', return_value=''), patch.object(M.shutil, 'rmtree', side_effect=PermissionError) as remove:
            result = M.cleanup_workspace('/tmp/owned', processes_stopped=True)
        self.assertEqual(result['state'], 'failed')
        self.assertEqual(result['type'], 'PermissionError')
        remove.assert_called_once_with(Path('/tmp/owned'))
        with patch.object(M.Path, 'read_text', side_effect=PermissionError), patch.object(M.shutil, 'rmtree') as remove:
            self.assertEqual(M.cleanup_workspace('/tmp/owned', processes_stopped=True)['state'], 'failed')
        remove.assert_not_called()

    def test_workspace_requires_confirmed_process_exit(self):
        with patch.object(M.Path, 'read_text') as inventory, patch.object(M.shutil, 'rmtree') as remove:
            result = M.cleanup_workspace('/tmp/owned')
        self.assertEqual(result['reason'], 'workspace-process-exit-unconfirmed')
        inventory.assert_not_called()
        remove.assert_not_called()

    def test_run_all_retains_case_and_directory_failures(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            index, rpc, output = root / 'index.json', root / 'rpc', root / 'result.json'
            index.write_text(json.dumps({'freeze': {'carriers': [
                {'kind': 'deb', 'sha256': 'frozen'}, {'kind': 'AppImage', 'sha256': 'frozen'}]}}))
            rpc.write_text('test')
            receipt = {'kind': 'deb', 'result': 'failed', 'cases': {},
                       'failure': {'stage': 'launch', 'type': 'ValueError'},
                       'cleanup': {'state': 'failed', 'forced': [], 'remaining': [],
                                   'remainingAssessment': 'observed', 'unknown': ['unobserved-adopted-descendant'], 'failure': {'stage': 'supervisor-close', 'type': 'ValueError'}}}
            case = Mock()
            case.run.return_value = receipt
            case.result = receipt
            with patch.dict(M.os.environ, {'GITHUB_ACTIONS': 'true'}, clear=True), patch.object(M.os, 'geteuid', return_value=1000), patch.object(M.subprocess, 'run'), patch.object(M, 'load_fixtures'), patch.object(M, 'carrier_path', return_value=rpc), patch.object(M, 'CarrierRun', return_value=case), patch.object(M, 'sha', return_value='frozen'), patch.object(M.tempfile, 'mkdtemp', return_value=str(root / 'workspace')), patch.object(M, 'cleanup_workspace', return_value={'state': 'failed', 'reason': 'workspace-cleanup-unconfirmed', 'type': 'PermissionError'}) as workspace_cleanup:
                with self.assertRaises(ValueError):
                    M.run_all(index, root, rpc, output)
                self.assertFalse(workspace_cleanup.call_args.kwargs['processes_stopped'])
            result = json.loads(output.read_text())
            carrier = result['carriers'][0]
            self.assertEqual(carrier['failure']['stage'], 'launch')
            self.assertEqual(carrier['cleanup']['failure']['stage'], 'supervisor-close')
            self.assertEqual(carrier['workspaceCleanup']['type'], 'PermissionError')
            self.assertEqual(result['result'], 'failed')

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

    def test_private_session_xml_is_closed_and_same_uid(self):
        import xml.etree.ElementTree as ET
        with tempfile.TemporaryDirectory(prefix='session-') as tmp:
            config, endpoint = M.private_session_config(Path(tmp))
            root = ET.fromstring(config.read_text())
            self.assertEqual([node.tag for node in root], ['type', 'keep_umask', 'listen', 'auth', 'policy'])
            self.assertEqual(root.findtext('listen'), 'unix:path=' + str(endpoint))
            self.assertEqual(root.findtext('auth'), 'EXTERNAL')
            self.assertEqual([node.attrib for node in root.find('policy')][:2], [{'user': '*'}, {'user': str(os.getuid())}])
            self.assertEqual([node.tag for node in root.find('policy')][:2], ['deny', 'allow'])
            self.assertEqual(config.stat().st_mode & 0o777, 0o600)
            self.assertEqual(config.parent.stat().st_mode & 0o777, 0o700)
            with self.assertRaises(FileExistsError):
                M.private_session_config(Path(tmp))
        source = Path(M.__file__).read_text()
        self.assertNotIn("'--session'", source)
        self.assertIn("module.verify(run=lambda argv: supervisor.query(", source)
        self.assertNotIn("DBUS_SYSTEM_BUS_ADDRESS'] =", source)

    def test_private_session_path_rejects_injection_or_long_socket(self):
        with tempfile.TemporaryDirectory() as tmp:
            for name in ('comma,service', 'xml&injection', 'x' * 110):
                directory = Path(tmp) / name
                directory.mkdir()
                with self.assertRaises(ValueError):
                    M.private_session_config(directory)

    def test_private_session_real_method_contract_and_failures(self):
        endpoint = Path('/tmp/private/bus')
        address = 'unix:path=/tmp/private/bus,guid=' + 'a' * 32
        good = ["(['org.freedesktop.DBus'],)", "('" + 'b' * 32 + "',)"]
        supervisor = Mock()
        with patch.object(Path, 'lstat', return_value=SimpleNamespace(st_mode=M.stat.S_IFSOCK | 0o600, st_uid=os.getuid())):
            supervisor.query.side_effect = good
            result = M.verify_private_session(supervisor, {}, address, endpoint)
            self.assertEqual(result['activation'], 'none')
            self.assertEqual(result['notTested'], M.PORTAL_NOT_TESTED)
            self.assertEqual(supervisor.query.call_count, 2)
            for empty in ('(@as [],)', '([],)'):
                supervisor.query.side_effect = [empty, good[1]]
                self.assertEqual(M.verify_private_session(supervisor, {}, address, endpoint)['activatableNames'], [])
            for output in ("(['org.freedesktop.portal.Documents'],)", "(['org.freedesktop.DBus', 'other'],)", "('not-a-list',)", "private malformed"):
                supervisor.query.side_effect = [output]
                with self.assertRaises((ValueError, SyntaxError)):
                    M.verify_private_session(supervisor, {}, address, endpoint)
            supervisor.query.reset_mock()
            for bad in ('unix:abstract=other', address + ';tcp:host=localhost', 'unix:path=/tmp/other,guid=' + 'a' * 32):
                with self.assertRaises(ValueError):
                    M.verify_private_session(supervisor, {}, bad, endpoint)
            supervisor.query.assert_not_called()
        with patch.object(Path, 'lstat', return_value=SimpleNamespace(st_mode=M.stat.S_IFREG, st_uid=os.getuid())):
            with self.assertRaises(ValueError):
                M.verify_private_session(supervisor, {}, address, endpoint)

    def test_bus_address_read_is_bounded_and_rejects_eof(self):
        stream = Mock()
        with patch.object(M.select, 'select', return_value=([stream], [], [])), patch.object(M.os, 'read', side_effect=[b'a', b'\n']):
            self.assertEqual(M.private_bus_address(stream), 'a')
        for blocks in ([b''], [b'x'] * 4096):
            with patch.object(M.select, 'select', return_value=([stream], [], [])), patch.object(M.os, 'read', side_effect=blocks):
                with self.assertRaises(ValueError):
                    M.private_bus_address(stream)
        with patch.object(M.select, 'select', return_value=([], [], [])):
            with self.assertRaises(ValueError):
                M.private_bus_address(stream)

    def probe_supervisor(self):
        supervisor = M.Supervisor.__new__(M.Supervisor)
        child = Mock(pid=913)
        child.poll.return_value = None
        child.wait.return_value = 0
        supervisor.spawn = Mock(return_value=child)
        supervisor.signal = Mock()
        supervisor.owned = {913: {'pid': 913, 'startTime': '17', 'uid': os.getuid()}}
        supervisor.forced, supervisor.unknown = [], []
        return supervisor, child

    def test_supervised_probe_success_reaped_without_signal(self):
        supervisor, child = self.probe_supervisor()
        with patch.object(M.select, 'select', return_value=([child.stdout], [], [])), patch.object(M.os, 'read', side_effect=[b'custody\n', b'']):
            self.assertEqual(supervisor.query(['/usr/bin/dpkg-query'], {}), 'custody\n')
        supervisor.spawn.assert_called_once_with(['/usr/bin/dpkg-query'], {}, 'tool-probe', stdout=M.subprocess.PIPE)
        supervisor.signal.assert_not_called()
        child.wait.assert_called_once()
        child.stdout.close.assert_called_once()

    def test_supervised_probe_timeout_output_and_exit_failure_reap(self):
        for failure in ('timeout', 'bound', 'exit', 'decode'):
            supervisor, child = self.probe_supervisor()
            child.wait.return_value = 1 if failure == 'exit' else 0
            reads = [b'abcdef'] if failure == 'bound' else [b'\xff', b''] if failure == 'decode' else [b'']
            ready = [] if failure == 'timeout' else [child.stdout]
            with patch.object(M.select, 'select', return_value=(ready, [], [])), patch.object(M.os, 'read', side_effect=reads):
                with self.assertRaises(ValueError):
                    supervisor.query(['probe'], {}, limit=5)
            supervisor.signal.assert_called_once_with(supervisor.owned[913], M.signal.SIGKILL)
            self.assertEqual(child.wait.call_args.kwargs['timeout'], 5)
            child.stdout.close.assert_called_once()

    def test_supervised_probe_unknown_signal_remains_failure(self):
        supervisor, child = self.probe_supervisor()
        supervisor.signal.side_effect = ValueError('UID changed')
        child.wait.side_effect = M.subprocess.TimeoutExpired('probe', 5)
        with patch.object(M.select, 'select', return_value=([], [], [])):
            with self.assertRaisesRegex(ValueError, 'timed out'):
                supervisor.query(['probe'], {})
        self.assertEqual(supervisor.unknown, ['tool-probe-signal-unconfirmed', 'tool-probe-wait-timeout'])
        child.stdout.close.assert_called_once()
        for kwargs in ({'timeout': 16}, {'limit': 16385}, {'limit': 0}):
            with self.assertRaises(ValueError):
                supervisor.query(['probe'], {}, **kwargs)

    def test_inventory_failure_closes_only_direct_ordinary_bus_still_unknown(self):
        supervisor, bus = self.probe_supervisor()
        supervisor.children = {913: bus, 914: Mock()}
        supervisor.roles = {913: 'dbus', 914: 'root-controller'}
        supervisor._close_owned = Mock(side_effect=ValueError('foreign UID'))
        supervisor.stop, supervisor.thread = Mock(), Mock()
        supervisor.thread.is_alive.return_value = False
        result = supervisor.close(True)
        supervisor.signal.assert_called_once_with(supervisor.owned[913], M.signal.SIGTERM)
        self.assertEqual(result['state'], 'failed')
        self.assertEqual(result['remainingAssessment'], 'unknown')
        self.assertIn('cleanup-inventory-unconfirmed', result['unknown'])
        supervisor.children[914].poll.assert_not_called()
        supervisor.thread.join.assert_called_once_with(2)


    def test_spawn_registers_direct_probe_under_lock_before_scan(self):
        import threading
        supervisor = M.Supervisor.__new__(M.Supervisor)
        supervisor.lock = threading.RLock()
        supervisor.children, supervisor.owned, supervisor.roles = {}, {}, {}
        child = Mock(pid=913)
        item = {'pid': 913, 'startTime': '17', 'uid': os.getuid()}
        with patch.object(M.subprocess, 'Popen', return_value=child), patch.object(M, 'process_identity', return_value=item):
            self.assertIs(supervisor.spawn(['probe'], {}, 'tool-probe'), child)
        self.assertIs(supervisor.children[913], child)
        self.assertEqual(supervisor.owned[913], item)
        self.assertEqual(supervisor.roles[913], 'tool-probe')

    def test_spawn_capture_failure_retains_handle_without_signal_authority(self):
        import threading
        for identity in (None, PermissionError('private proc denied')):
            for exited in (True, False):
                supervisor = M.Supervisor.__new__(M.Supervisor)
                supervisor.lock = threading.RLock()
                supervisor.children, supervisor.owned, supervisor.roles = {}, {}, {}
                supervisor.unknown = []
                supervisor.signal = Mock()
                child = Mock(pid=913)
                child.poll.return_value = 0 if exited else None
                with patch.object(M.subprocess, 'Popen', return_value=child), patch.object(M, 'process_identity', side_effect=identity if isinstance(identity, Exception) else None, return_value=identity):
                    with self.assertRaises((ValueError, PermissionError)):
                        supervisor.spawn(['probe'], {}, 'tool-probe', stdout=M.subprocess.PIPE)
                self.assertIs(supervisor.children[913], child)
                self.assertEqual(supervisor.owned, {})
                self.assertEqual(supervisor.roles, {})
                self.assertIn('spawn-identity-unconfirmed', supervisor.unknown)
                supervisor.signal.assert_not_called()
                child.stdout.close.assert_called_once()
                child.stderr.close.assert_called_once()
                if exited:
                    child.wait.assert_called_once_with(timeout=0)
                else:
                    child.wait.assert_not_called()

    def test_spawn_secondary_reap_pipe_failures_preserve_capture_failure(self):
        import threading
        supervisor = M.Supervisor.__new__(M.Supervisor)
        supervisor.lock = threading.RLock()
        supervisor.children, supervisor.owned, supervisor.roles = {}, {}, {}
        supervisor.unknown = []
        child = Mock(pid=913)
        child.poll.return_value = 0
        child.wait.side_effect = M.subprocess.TimeoutExpired('probe', 0)
        child.stdout.close.side_effect = OSError('pipe private detail')
        with patch.object(M.subprocess, 'Popen', return_value=child), patch.object(M, 'process_identity', side_effect=PermissionError('capture failure')):
            with self.assertRaisesRegex(PermissionError, 'capture failure'):
                supervisor.query(['probe'], {})
        self.assertEqual(supervisor.unknown, ['spawn-identity-unconfirmed', 'spawn-reap-unconfirmed', 'spawn-pipe-close-unconfirmed'])
        child.stderr.close.assert_called_once()
        self.assertEqual(supervisor.owned, {})

    def test_verified_tools_injects_only_supervised_fixed_environment_probe(self):
        supervisor, module, spec = Mock(), Mock(), Mock()
        module.verify.side_effect = lambda *, run: run(['/usr/bin/dpkg-query', '-S', '/usr/bin/WebKitWebDriver'])
        with patch.object(M.importlib.util, 'spec_from_file_location', return_value=spec), patch.object(M.importlib.util, 'module_from_spec', return_value=module):
            M.verified_tools(supervisor)
        supervisor.query.assert_called_once_with(['/usr/bin/dpkg-query', '-S', '/usr/bin/WebKitWebDriver'], {'PATH': '/usr/bin:/bin', 'LC_ALL': 'C'})

    def test_inventory_fallback_cannot_bypass_uid_rejection(self):
        supervisor, bus = self.probe_supervisor()
        supervisor.children, supervisor.roles = {913: bus}, {913: 'dbus'}
        supervisor._close_owned = Mock(side_effect=ValueError('inventory rejected'))
        supervisor.signal.side_effect = ValueError('current uid root')
        supervisor.stop, supervisor.thread = Mock(), Mock()
        supervisor.thread.is_alive.return_value = False
        result = supervisor.close(True)
        self.assertIn('private-session-close-unconfirmed', result['unknown'])
        self.assertEqual(result['state'], 'failed')
        self.assertEqual(result['remainingAssessment'], 'unknown')
        bus.wait.assert_not_called()
        supervisor.thread.join.assert_called_once_with(2)


if __name__ == '__main__':
    unittest.main()
