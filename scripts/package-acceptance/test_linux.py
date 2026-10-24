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
        run.result = {}
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
        run.result = {}
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
        run.result = {}
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
        driver.request = Mock(return_value={'schema': M.NATIVE_BRIDGE_SCHEMA, 'state': 'envelope-rejected', 'data': None, 'diagnostic': 'unknown'})
        with self.assertRaises(M.NativeRejected):
            driver.invoke('config_save', {})
        driver.request.return_value = {'schema': M.NATIVE_BRIDGE_SCHEMA, 'state': 'promise-rejected', 'data': None, 'diagnostic': 'none'}
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
        self.assertIn("http.client.HTTPConnection('127.0.0.1', self.port", source)
        self.assertNotIn('urllib.request', source)
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


    def api_fixture(self, port=41321):
        core = {'pid': 77, 'startTime': '125', 'uid': os.getuid(), 'sha256': 'a' * 64, 'exe': '/private/sing-box'}
        secret = 'private-test-secret-never-public'
        graph = {'services': [{'type': 'api', 'listen': '127.0.0.1', 'listen_port': port, 'secret': secret}]}
        status = {'running': True, 'pid': 77, 'clashApiPort': port}
        return core, secret, graph, status

    def test_dynamic_api_binder_matches_actual_status_not_legacy_control(self):
        core, secret, graph, status = self.api_fixture()
        graph['controlPort'] = 52345
        endpoint = M.bind_api_endpoint(graph, status, secret, core)
        self.assertEqual(endpoint['port'], 41321)
        self.assertEqual(endpoint['startTime'], '125')
        self.assertNotIn(secret, json.dumps(endpoint))
        self.assertNotIn('secret', endpoint)

    def test_api_binding_rejects_all_foreign_or_malformed_endpoints(self):
        import copy
        core, secret, graph, status = self.api_fixture()
        mutations = [lambda g,s: g['services'].append(copy.deepcopy(g['services'][0])),
            lambda g,s: g['services'].clear(), lambda g,s: g['services'][0].update(listen='0.0.0.0'),
            lambda g,s: g['services'][0].update(secret='wrong'), lambda g,s: s.update(pid=78),
            lambda g,s: s.update(pid=True), lambda g,s: s.update(running=False),
            lambda g,s: s.update(starting=True), lambda g,s: s.pop('clashApiPort')]
        for port in (None, True, 0, -1, 65536, '41321', 41322):
            mutations.append(lambda g,s,p=port: s.update(clashApiPort=p))
        for port in (None, True, 0, -1, 65536, '41321'):
            mutations.append(lambda g,s,p=port: g['services'][0].update(listen_port=p))
        for mutate in mutations:
            g,s = copy.deepcopy(graph), dict(status); mutate(g,s)
            with self.assertRaises(ValueError): M.bind_api_endpoint(g,s,secret,core)

    def test_api_capture_uses_exact_private_content_and_brackets_status(self):
        core, secret, graph, status = self.api_fixture()
        run = M.CarrierRun.__new__(M.CarrierRun)
        run.core, run.secret, run.runtime_path, run.home, run.workspace = core, secret, Path('/tmp/runtime'), Path('/tmp'), Path('/tmp')
        proof = {'sha256': 'a'*64, 'pid': 77, 'startTime': '125'}
        run.status = Mock(return_value=status); run.runtime_identity = Mock(return_value=dict(proof))
        with patch.object(M, 'stable_runtime_capture', return_value=(proof, graph)) as capture:
            endpoint, result, content = run.observed_api_endpoint()
            self.assertIs(content, graph); self.assertEqual(endpoint['port'], 41321)
            self.assertTrue(capture.call_args.kwargs['include_content'])
            for field, value in (('sha256', 'b'*64), ('pid', 78), ('startTime', '126')):
                run.runtime_identity.return_value = {**proof, field: value}
                with self.assertRaises(ValueError): run.observed_api_endpoint()

    def test_endpoint_revalidation_allows_apply_graph_change_not_endpoint_or_birth(self):
        run = M.CarrierRun.__new__(M.CarrierRun)
        core, secret, graph, status = self.api_fixture()
        run.api_endpoint = M.bind_api_endpoint(graph, status, secret, core)
        run.observed_api_endpoint = Mock(return_value=(dict(run.api_endpoint), {'sha256': 'new-hot-graph'}, {}))
        self.assertEqual(run.revalidate_api_endpoint()['sha256'], 'new-hot-graph')
        for field, value in (('port', 41322), ('pid', 78), ('startTime', '126')):
            run.observed_api_endpoint.return_value = ({**run.api_endpoint, field:value}, {}, {})
            with self.assertRaises(ValueError): run.revalidate_api_endpoint()
        run.api_endpoint = None
        with self.assertRaises(ValueError): run.revalidate_api_endpoint()

    def test_connect_diagnostics_are_fixed_bounded_and_secret_free(self):
        run = M.CarrierRun.__new__(M.CarrierRun); run.result = {}; run.api_endpoint = {'old': True}
        run.connect_phase('stopped-check')
        self.assertIsNone(run.api_endpoint)
        status = {'running': True, 'starting': False, 'pid': 77, 'clashApiPort': 41321,
                  'mainGeneration': 5, 'mixedPort': 41234, 'secret': 'private-test-secret', 'config': 'never public'}
        run.status = Mock(return_value=status)
        for _ in range(500): run.connect_status()
        for phase in M.CONNECT_PHASES[1:]: run.connect_phase(phase, status)
        observation = run.result['connectObservation']
        self.assertEqual(len(observation['phases']), len(M.CONNECT_PHASES))
        self.assertEqual(observation['lastStatus']['clashApiPort'], 41321)
        self.assertNotIn('private-test-secret', json.dumps(observation))
        self.assertNotIn('never public', json.dumps(observation))
        with self.assertRaises(ValueError): run.connect_phase('untrusted stage')
        self.assertEqual(M.safe_connect_status({'pid': True, 'mainGeneration': -1, 'starting': 'secret'}), {})

    def test_actual_producer_serialization_and_existing_probe_ownership_contract(self):
        source = lambda path: (SOURCE / path).read_text()
        self.assertIn('clash_api_port: api_port,', source('src-tauri/src/runtime/proxy/startup.rs'))
        self.assertIn('tailscale_api_port: api_port,', source('src-tauri/src/runtime/proxy/startup.rs'))
        self.assertIn('listen_port: deps.tailscale_api_port,', source('crates/config-engine/src/builder/generate.rs'))
        self.assertIn('#[serde(rename = "clashApiPort", default)]', source('src-tauri/src/runtime/proxy.rs'))
        self.assertIn('fn control_port(&self) -> Option<u16> {\n        None\n', source('crates/config-engine/src/builder/inbounds.rs'))
        probe = source('crates/singbox-grpc/examples/package_runtime_probe.rs')
        for guard in ('owned-loopback-listener-not-unique', 'listener-not-owned-by-core', 'owned-process-changed', 'owned-process-uid-differs'):
            self.assertIn(guard, probe)
        shared = Path(M.__file__).read_text()
        self.assertNotIn("'port': self.control", shared)
        self.assertIn("self.api_endpoint['port']", shared)

    def test_plain_connect_rebinds_restart_and_requires_owned_probe(self):
        core, secret, graph, status = self.api_fixture()
        run = M.CarrierRun.__new__(M.CarrierRun)
        run.result = {'cases': {'default_dashboard': {'evidence': {}}}}
        run.secret, run.home, run.workspace, run.payload = secret, Path('/tmp'), Path('/tmp'), Path('/payload')
        run.members = {'core': {'path': 'sing-box'}, 'cronet': {'path': 'libcronet.so', 'sha256':'b'*64}}
        run.app = {'pid': 70, 'startTime': '100', 'sha256': 'c'*64}
        run.status = Mock(return_value={'running':False}); run.driver = Mock(); run.supervisor = Mock()
        run.supervisor.executable.return_value = core
        endpoint = M.bind_api_endpoint(graph,status,secret,core)
        run.observed_api_endpoint = Mock(return_value=(endpoint, {'sha256':'d'*64}, graph))
        run.runtime_identity = Mock(return_value={'sha256':'d'*64}); run.rpc = Mock()
        with patch.object(M, 'wait_for', return_value=status), patch.object(M, 'regular', side_effect=lambda p,*a: p), patch.object(Path, 'read_bytes', return_value=b'core\0run\0-c\0/tmp/singbox-runtime.json\0'), patch.object(Path, 'read_text', return_value=''), patch.object(M, 'cronet_mapping', return_value={}):
            run.connect()
            self.assertTrue(run.result['apiEndpoint']['ownedListenerProbe'])
            self.assertEqual(run.api_endpoint['port'], 41321)
            run.rpc.assert_called_once_with('capabilities')
            changed = {**endpoint,'port':41322}
            run.observed_api_endpoint.return_value = (changed, {'sha256':'e'*64}, graph)
            run.connect()
            self.assertEqual(run.api_endpoint['port'],41322)
            run.rpc.side_effect = ValueError('listener-not-owned-by-core')
            with self.assertRaises(ValueError): run.connect()
            self.assertEqual(run.result['connectObservation']['phase'], 'owned-api-probe')


    def test_plain_rpc_uses_actual_bound_endpoint_and_revalidates_after_probe(self):
        import threading
        core, secret, graph, status = self.api_fixture()
        run = M.CarrierRun.__new__(M.CarrierRun)
        run.core, run.secret = core, secret
        run.api_endpoint = M.bind_api_endpoint(graph,status,secret,core)
        run.fixture_control_port = 52345
        run.revalidate_api_endpoint = Mock()
        run.runtime_path, run.rpc_client, run.env = Path('/tmp/singbox-runtime.json'), Path('/private/probe'), {}
        run.supervisor = SimpleNamespace(lock=threading.RLock(), children={},owned={},roles={})
        child = Mock(pid=99, returncode=0)
        child.communicate.return_value = (json.dumps({'schema':'polaris-package-rpc-probe-v1', 'ok':True, 'action':'capabilities', 'result':{'active':{}}}).encode(), None)
        with patch.object(M, 'process_identity', side_effect=lambda pid: core if pid==77 else {'pid':99,'startTime':'200'}), patch.object(M.subprocess,'Popen',return_value=child) as popen:
            self.assertEqual(run.rpc('capabilities'), {'active':{}})
            sent = json.loads(child.communicate.call_args.args[0])
            self.assertEqual(sent['port'],41321); self.assertNotEqual(sent['port'],run.fixture_control_port)
            self.assertEqual(sent['secret'],secret)
            self.assertNotIn(secret,str(popen.call_args))
            self.assertEqual(run.revalidate_api_endpoint.call_count,2)
            run.revalidate_api_endpoint.side_effect = [None, ValueError('birth changed after call')]
            with self.assertRaises(ValueError): run.rpc('capabilities')

    def test_every_connect_phase_failure_keeps_fixed_redacted_phase(self):
        core, secret, graph, status = self.api_fixture()
        for failed_phase in M.CONNECT_PHASES:
            run = M.CarrierRun.__new__(M.CarrierRun)
            run.result = {'cases':{'default_dashboard':{'evidence':{}}}}
            run.secret, run.home, run.workspace, run.payload = secret, Path('/tmp'), Path('/tmp'), Path('/payload')
            run.members = {'core':{'path':'sing-box'},'cronet':{'path':'libcronet.so','sha256':'b'*64}}
            run.app = {'pid':70,'startTime':'100','sha256':'c'*64}
            run.status = Mock(return_value={'running':False}); run.driver = Mock(); run.supervisor = Mock()
            run.supervisor.executable.return_value = core
            endpoint = M.bind_api_endpoint(graph,status,secret,core)
            run.observed_api_endpoint = Mock(return_value=(endpoint,{},graph))
            run.runtime_identity = Mock(return_value={}); run.rpc = Mock()
            original = run.connect_phase
            def phase(name, status=None):
                original(name,status)
                if name == failed_phase: raise ValueError('fixture private error')
            run.connect_phase = phase
            with patch.object(M,'wait_for',return_value=status), patch.object(M,'regular',side_effect=lambda p,*a:p), patch.object(Path,'read_bytes',return_value=b'core\0run\0-c\0/tmp/singbox-runtime.json\0'), patch.object(Path,'read_text',return_value=''), patch.object(M,'cronet_mapping',return_value={}):
                with self.assertRaises(ValueError): run.connect()
            self.assertEqual(run.result['connectObservation']['phase'],failed_phase)
            self.assertNotIn(secret,json.dumps(run.result))
            self.assertNotIn('fixture private error',json.dumps(run.result))
            self.assertNotIn('execution',run.result)
            self.assertNotIn('apiEndpoint',run.result)



class GuiNavigationContracts(unittest.TestCase):
    def run_fixture(self, expanded='false'):
        run = M.CarrierRun.__new__(M.CarrierRun)
        run.result = {}; run.driver = Mock(); run.text = lambda key: key
        config = {'servers': [{'id': 'node-a', 'name': 'Loop A', 'subscriptionId': 'package-fixture'},
                              {'id': 'node-b', 'name': 'Loop B', 'subscriptionId': 'package-fixture'}],
                  'subscriptions': [{'id': 'package-fixture', 'name': 'Package loopback'}],
                  'selectedServerId': 'node-b', 'selectionIntent': None}
        run.config = Mock(return_value=config); run.status = Mock(return_value={'running': False})
        run.driver.element.return_value = None; run.driver.execute.return_value = 'false'
        state = {'expanded': expanded}
        def control(kind, label):
            return {'element': {M.ELEMENT: kind}, 'expanded': state['expanded'], 'checked': 'false'}
        run.driver.contract_control.side_effect = control
        run.driver.wait_control.side_effect = control
        def click(value):
            if value['element'][M.ELEMENT] == 'fixture-group': state['expanded'] = 'true'
        run.driver.click_control.side_effect = click
        return run

    def test_folded_group_expands_once_then_native_option_click(self):
        run = self.run_fixture(); run.pick('B')
        self.assertEqual([c.args[0]['element'][M.ELEMENT] for c in run.driver.click_control.call_args_list],
                         ['fixture-group', 'fixture-node'])
        self.assertEqual(run.result['guiObservation']['phase'], 'pick-complete')

    def test_expanded_group_never_toggled(self):
        run = self.run_fixture('true'); run.pick('B')
        self.assertEqual(run.driver.click_control.call_count, 1)
        self.assertEqual(run.driver.click_control.call_args.args[0]['element'][M.ELEMENT], 'fixture-node')

    def test_open_menu_not_toggled_closed(self):
        run = self.run_fixture(); run.driver.execute.return_value = 'true'; run.pick('B')
        run.driver.click.assert_not_called()

    def test_wrong_fixture_identity_and_duplicate_label_never_click(self):
        for variant in ('name','group','duplicate','node'):
            run = self.run_fixture(); config=run.config.return_value
            if variant=='name': config['subscriptions'][0]['name']='other private group'
            if variant=='group': config['subscriptions'][0]['id']='other'
            if variant=='duplicate': config['servers'].append({'id':'third','name':'Loop B'})
            if variant=='node': config['servers'][1]['name']='other'
            with self.assertRaises(M.GuiFailure): run.pick('B')
            run.driver.click.assert_not_called(); run.driver.click_control.assert_not_called()

    def test_unknown_expansion_no_toggle(self):
        run=self.run_fixture(None)
        with self.assertRaises(M.GuiFailure): run.pick('B')
        run.driver.click_control.assert_not_called()

    def test_ambiguous_click_failure_never_retried(self):
        run=self.run_fixture(); run.driver.click_control.side_effect=M.GuiFailure('webdriver-http-error',500)
        with self.assertRaises(M.GuiFailure): run.pick('B')
        self.assertEqual(run.driver.click_control.call_count,1)
        self.assertEqual(run.result['guiObservation']['phase'],'pick-group-expand')

    def test_actual_node_unknown_remains_failure_after_ui_selection(self):
        run=self.run_fixture('true'); run.status.return_value={'running':True,'actualServerId':None}
        original_read = run.gui_read
        with patch.object(M, 'wait_for', side_effect=lambda read, condition=bool, **kw: read() if condition(read()) else (_ for _ in ()).throw(M.GuiFailure('observation-timeout'))):
            with self.assertRaises(M.GuiFailure): run.pick('B')
        self.assertIsNone(run.result['guiObservation']['lastState']['actualServerId'])

    def test_control_rejects_duplicate_hidden_disabled_and_bad_identity(self):
        valid={'count':1,'visible':True,'disabled':False,'element':{M.ELEMENT:'control'}}
        for change in ({'count':2},{'count':0},{'visible':False},{'disabled':True},{'element':{}}):
            driver=M.WebDriver(1); driver.execute=Mock(return_value={**valid,**change})
            with self.assertRaises(M.GuiFailure): driver.contract_control('preference-switch','fixed')
        driver=M.WebDriver(1); driver.execute=Mock(return_value=valid)
        self.assertEqual(driver.contract_control('preference-switch','fixed'),valid)
        script=driver.execute.call_args.args[0]
        self.assertIn('.sr-title > b',script); self.assertIn('[role="switch"]',script)
        self.assertNotIn('button[role=switch]',script); self.assertIn("getAttribute('aria-disabled')",script)
        primitives=(SOURCE/'ui/src/components/screens/settings/Primitives.tsx').read_text()
        self.assertIn('className="sr-title"',primitives)
        self.assertRegex(primitives,r'<span\s+id=\{id\}\s+role="switch"')

    def test_preference_shared_real_span_control_and_readback(self):
        run=self.run_fixture()
        run.preference_pending_save=Mock(side_effect=[False,True,False])
        run.preference_disk=Mock(side_effect=[{'value':False},{'value':True}])
        run.driver.wait_control.side_effect=[{'checked':'false','element':{M.ELEMENT:'preference-switch'}},{'checked':'true','element':{M.ELEMENT:'preference-switch'}}]
        run.driver.observations.side_effect=[[],[{'sequence':1,'source':'original-config-save-callback',
            'success':True,'status':'saved','versionValid':True,'preference':True}]]
        run.preference(True)
        self.assertEqual(run.driver.wait_control.call_count,2)
        self.assertEqual(run.driver.click_control.call_count,1)
        run.config.assert_not_called()
        self.assertEqual(run.result['guiObservation']['phase'],'preference-return')

    def test_observations_bounded_and_secret_free(self):
        run=self.run_fixture()
        for n in range(200): run.gui_phase('pick-status-readback',{'running':bool(n % 2),'actualServerId':'secret','clashApiSecret':'secret'})
        result=run.result['guiObservation']; self.assertEqual(len(result['steps']),128); self.assertTrue(result['truncated'])
        self.assertNotIn('secret',json.dumps(result)); self.assertEqual(result['lastState']['actualServerId'],'other')
        self.assertEqual(M.gui_failure(ValueError('secret')),{'code':'unclassified-contract-failure'})

    def test_echo_pending_failure_and_validated_evidence(self):
        run=self.run_fixture(); run.fixtures=Mock()
        run.fixtures.exchange.side_effect=ValueError('private payload')
        with self.assertRaises(ValueError): run.exchange(Mock(),'A')
        row=run.result['streamObservation']['exchanges'][0]
        self.assertEqual(row['state'],'failed'); self.assertNotIn('private',json.dumps(row))
        self.assertEqual(row['bytes'],36); self.assertEqual(len(row['nonceSha256']),64)
        run.fixtures.exchange.side_effect=None;run.fixtures.exchange.return_value={'backend':'A'}
        self.assertEqual(run.exchange(Mock(),'A'),{'backend':'A'})
        self.assertTrue(run.result['streamObservation']['exchanges'][1]['exactEchoMatched'])

    def test_wait_control_only_retries_missing_read_not_click_or_ambiguity(self):
        driver=M.WebDriver(1)
        valid={'count':1,'visible':True,'disabled':False,'element':{M.ELEMENT:'x'}}
        driver.execute=Mock(side_effect=[{'count':0},valid]);driver.request=Mock()
        with patch.object(M.time,'sleep'):
            self.assertEqual(driver.wait_control('fixture-group','fixed'),valid)
        self.assertEqual(driver.execute.call_count,2);driver.request.assert_not_called()
        driver.execute=Mock(return_value={'count':2})
        with self.assertRaises(M.GuiFailure):driver.wait_control('fixture-group','fixed')
        self.assertEqual(driver.execute.call_count,1);driver.request.assert_not_called()

    def test_preference_wrong_checked_state_is_not_blindly_toggled(self):
        run=self.run_fixture();run.config.return_value={'interruptConnectionsOnSwitch':False}
        run.driver.wait_control.side_effect=None
        run.driver.wait_control.return_value={'element':{M.ELEMENT:'switch'},'checked':'true'}
        with self.assertRaises(M.GuiFailure):run.preference(True)
        run.driver.click_control.assert_not_called()

    def test_actual_locator_script_on_bounded_dom_fixture(self):
        import subprocess
        driver=M.WebDriver(1)
        driver.execute=Mock(return_value={'count':1,'visible':True,'disabled':False,'element':{M.ELEMENT:'x'}})
        driver.contract_control('fixture-group','Package loopback')
        script=driver.execute.call_args.args[0]
        harness=r'''
const input=JSON.parse(require('fs').readFileSync(0,'utf8'));
const make=(row)=>({tagName:'BUTTON',disabled:row.disabled===true,
 getClientRects:()=>row.hidden?[]:[{}],getAttribute:k=>row[k]??null,
 children:[{tagName:'SPAN',classList:{contains:()=>false},textContent:row.label},
 {tagName:'SPAN',classList:{contains:()=>true},textContent:'2'}],
 querySelectorAll:s=>s==='.nm-name'||s==='.sr-title > b'?[{textContent:row.label}]:s==='[role="switch"]'?[make({...row,switch:true})]:[],
 marker:row.switch?'span-switch':'button'});
const elements=input.rows.map(make);
const doc={querySelectorAll:s=>elements};
const run=new Function('document','getComputedStyle','return function(){'+input.script+'}')(doc,()=>({visibility:'visible'}));
const value=run(input.kind,input.label);
if(value.element)value.element={marker:value.element.marker};
process.stdout.write(JSON.stringify(value));
'''
        for kind in ('fixture-group','fixture-node','preference-switch'):
            for rows,count,disabled in [([{'label':'fixed'}],1,False),
                    ([{'label':'fixed'},{'label':'fixed'}],2,None),
                    ([{'label':'other'}],0,None),
                    ([{'label':'fixed','aria-disabled':'true'}],1,True)]:
                value=json.loads(subprocess.run(['node','-e',harness],input=json.dumps(
                    {'script':script,'kind':kind,'label':'fixed','rows':rows}),text=True,capture_output=True,check=True).stdout)
                self.assertEqual(value['count'],count)
                if count==1:
                    self.assertEqual(value['disabled'],disabled)
                    if kind=='preference-switch':self.assertEqual(value['element']['marker'],'span-switch')

    def test_webdriver_error_categories_discard_remote_content(self):
        import io
        for remote, expected in [('element click intercepted','element click intercepted'),('secret-value','unknown'),({'secret':'value'},'unknown')]:
            driver=M.WebDriver(1)
            raw=json.dumps({'value':{'error':remote,'message':'SECRET','stacktrace':'SECRET'}}).encode()
            response=Mock(status=400, length=0); response.isclosed.return_value=False; response.read1.side_effect=[raw,b'']
            connection=Mock(); connection.getresponse.return_value=response
            with patch.object(M.http.client,'HTTPConnection',return_value=connection):
                with self.assertRaises(M.GuiFailure) as raised:driver.request('POST','/safe',{})
            self.assertEqual(M.gui_failure(raised.exception),{'code':'webdriver-http-error','httpStatus':400,'protocol':expected})
            self.assertNotIn('SECRET',str(raised.exception))

    def test_render_matrix_dialog_auto_and_quit_contract(self):
        home=(SOURCE/'ui/src/components/screens/home/HomeScreen.tsx').read_text()
        menu=(SOURCE/'ui/src/components/screens/home/NodeMenu.tsx').read_text()
        modal=(SOURCE/'ui/src/components/dialogs/Modal.tsx').read_text()
        restart=(SOURCE/'ui/src/components/dialogs/RestartApplyDialog.tsx').read_text()
        auto=(SOURCE/'ui/src/mobile/AutoSelectControls.tsx').read_text()
        self.assertIn('aria-expanded={nodeMenuOpen}',home)
        self.assertIn('aria-expanded={isOpen}',menu)
        self.assertIn('isOpen && items.map',menu)
        self.assertIn('<dialog',modal);self.assertIn('titleId="restart-apply-title"',restart)
        self.assertIn('data-auto-select-scope={target.scope}',auto)
        self.assertIn('className="auto-choice-pick"',auto)
        source=Path(M.__file__).read_text()
        self.assertIn('dialog[open][aria-labelledby="restart-apply-title"] button',source)
        self.assertNotIn('button[role=switch]',source)
        self.assertNotIn('dispatchEvent(',source)


class DriverObservationTests(unittest.TestCase):
    def test_disconnect_is_retained_without_retry_and_later_read_does_not_erase_it(self):
        import http.client
        driver = M.WebDriver(1); driver.session = 'SECRET-SESSION'
        with patch.object(driver, '_request', side_effect=[http.client.RemoteDisconnected('SECRET-REMOTE'), 'synthetic-image']) as request:
            with self.assertRaises(http.client.RemoteDisconnected):
                driver.invoke('proxy_get_status', {'secret': 'SECRET-ARG'})
            self.assertEqual(request.call_count, 1)
            self.assertEqual(driver.request('GET', driver.path('/screenshot')), 'synthetic-image')
        first = driver.observation['firstFailure']
        self.assertEqual((first['route'], first['command'], first['error']),
                         ('/execute/async', 'proxy_get_status', 'RemoteDisconnected'))
        self.assertEqual(first['callback'], 'not-observed')
        self.assertEqual(driver.observation['requests'][1]['state'], 'decoded')
        self.assertNotIn('SECRET', json.dumps(driver.observation))
        self.assertIsNone(driver._observed_command)

    def test_http_error_retains_status_and_whitelisted_protocol_without_remote_text(self):
        import io
        driver = M.WebDriver(1); driver.session = 'SECRET-SESSION'
        response = Mock(status=500, length=0); response.isclosed.return_value = False
        response.read1.side_effect = [json.dumps({'value': {'error': 'script timeout', 'message': 'SECRET'}}).encode(), b'']
        connection = Mock(); connection.getresponse.return_value = response
        with patch.object(M.http.client, 'HTTPConnection', return_value=connection):
            with self.assertRaises(M.GuiFailure): driver.invoke('proxy_get_status')
        row = driver.observation['firstFailure']
        self.assertEqual((row['httpStatus'], row['protocol']), (500, 'script timeout'))
        self.assertNotIn('SECRET', json.dumps(driver.observation))

    def test_stopped_bridge_read_is_not_treated_as_a_transport_failure(self):
        driver = M.WebDriver(1); driver.session = 'private'
        status = {'running': False, 'actualServerId': None}
        with patch.object(driver, '_request', return_value={'schema': M.NATIVE_BRIDGE_SCHEMA, 'state': 'returned', 'data': status, 'diagnostic': 'none'}):
            self.assertEqual(driver.invoke('proxy_get_status'), status)
        row = driver.observation['requests'][0]
        self.assertEqual((row['state'], row['callback']), ('decoded', 'returned'))
        self.assertNotIn('firstFailure', driver.observation)

    def test_observation_budget_keeps_first_failure_beyond_request_budget(self):
        import http.client
        driver = M.WebDriver(1); driver.session = 'private'
        with patch.object(driver, '_request', return_value=None):
            for _ in range(130): driver.request('GET', '/status')
        with patch.object(driver, '_request', side_effect=http.client.RemoteDisconnected()):
            with self.assertRaises(http.client.RemoteDisconnected): driver.request('GET', '/status')
        self.assertEqual(len(driver.observation['requests']), 128)
        self.assertTrue(driver.observation['truncated'])
        self.assertEqual(driver.observation['firstFailure']['error'], 'RemoteDisconnected')

    def test_failure_process_snapshot_never_adopts_signals_or_records_raw_process_fields(self):
        import threading
        item = {'pid': 123, 'startTime': '42', 'uid': 1001, 'ppid': 1, 'cmdline': 'SECRET'}
        child = Mock(); child.poll.return_value = None
        run = M.CarrierRun.__new__(M.CarrierRun); run.result = {}; run.driver = M.WebDriver(1)
        run.supervisor = SimpleNamespace(lock=threading.RLock(), owned={123:item},
                                        roles={123:'driver'}, children={123:child})
        with patch.object(M, 'process_identity', return_value=item): run.capture_driver_failure()
        row = run.result['failureProcessObservation']['processes'][0]
        self.assertEqual(row, {'pid':123,'startTime':'42','role':'driver','identity':'same-birth-uid','childState':'running'})
        self.assertNotIn('SECRET', json.dumps(run.result))
        child.kill.assert_not_called(); child.terminate.assert_not_called()
        with patch.object(M, 'process_identity', side_effect=OSError('SECRET')): run.capture_driver_failure()
        self.assertEqual(run.result['failureProcessObservation']['processes'][0]['identity'], 'unknown')
        self.assertNotIn('SECRET', json.dumps(run.result))


class NativeBridgeNamespaceTests(unittest.TestCase):
    def test_actual_http_200_native_envelopes_do_not_collide_with_protocol(self):
        import http.server
        import threading
        class Handler(http.server.BaseHTTPRequestHandler):
            value = None
            count = 0
            def do_POST(self):
                type(self).count += 1
                self.rfile.read(int(self.headers.get('Content-Length', 0)))
                data = json.dumps({'value': type(self).value}).encode()
                self.send_response(200); self.send_header('Content-Length', str(len(data)))
                self.end_headers(); self.wfile.write(data)
            def log_message(self, *args): pass
        server = http.server.HTTPServer(('127.0.0.1', 0), Handler)
        thread = threading.Thread(target=server.serve_forever, daemon=True); thread.start()
        try:
            d = M.WebDriver(server.server_port); d.session = 'private'
            for cmd in ('config_get', 'config_save', 'proxy_get_status', 'proxy_runtime_apply_status', 'auto_select_status'):
                Handler.value = {'schema': M.NATIVE_BRIDGE_SCHEMA, 'state': 'envelope-rejected', 'data': None, 'diagnostic': 'unknown'}
                before = Handler.count
                with self.subTest(cmd=cmd), self.assertRaises(M.NativeRejected): d.invoke(cmd)
                self.assertEqual(Handler.count, before + 1)
                self.assertEqual(d.observation['requests'][-1]['nativeBridge'], 'envelope-rejected')
            self.assertEqual(d.observation['firstNativeFailure']['nativeBridge'], 'envelope-rejected')
            self.assertNotIn('firstFailure', d.observation)
            # Retain fail-closed handling of an actual protocol-shaped error too.
            Handler.value = {'error': 'javascript error', 'message': 'SECRET'}
            with self.assertRaises(M.GuiFailure) as caught: d.invoke('proxy_get_status')
            self.assertEqual(caught.exception.protocol, 'javascript error')
            self.assertNotIn('SECRET', json.dumps(d.observation))
        finally:
            server.shutdown(); thread.join(timeout=2); server.server_close()

    def test_every_fixed_bridge_failure_and_polluted_envelope_fails_without_retry(self):
        for state in ('unavailable', 'invalid-envelope', 'promise-rejected', 'invoke-threw', 'SECRET'):
            d = M.WebDriver(1); d.session = 'private'
            with patch.object(d, '_request', return_value={'schema': M.NATIVE_BRIDGE_SCHEMA, 'state': state, 'data': None, 'diagnostic': 'none'}) as req:
                with self.assertRaises(M.GuiFailure) as caught: d.invoke('proxy_get_status')
                self.assertEqual(req.call_count, 1)
                self.assertNotIn('SECRET', json.dumps(M.gui_failure(caught.exception)))
        for bad in ({'ok': True, 'data': None}, {'schema': M.NATIVE_BRIDGE_SCHEMA, 'state': 'returned', 'data': None, 'secret': 1},
                    {'schema': M.NATIVE_BRIDGE_SCHEMA, 'state': 'promise-rejected', 'data': 'SECRET'}):
            self.assertEqual(M.native_bridge_state(bad), 'invalid-bridge')

    def test_real_javascript_bridge_all_native_outcomes(self):
        import subprocess
        # Runs the exact JS hook with real promises, no IPC/product/browser.
        cases = [
            ('undefined', 'unavailable'),
            ('()=>{throw Error("SECRET")}', 'invoke-threw'),
            ('()=>42', 'invalid-envelope'),
            ('()=>Promise.reject(Error("SECRET"))', 'promise-rejected'),
            ('()=>Promise.resolve(null)', 'invalid-envelope'),
            ('()=>Promise.resolve({success:false,error:"SECRET"})', 'envelope-rejected'),
            ('()=>Promise.resolve({success:true,data:{running:false,actualServerId:null}})', 'returned'),
        ]
        for invoke, expected in cases:
            script = ('global.window={__TAURI_INTERNALS__:{invoke:' + invoke + '}};'
                      'const fn=new Function(' + json.dumps(M.NATIVE_BRIDGE_JS) + ');'
                      'fn("proxy_get_status",{},v=>process.stdout.write(JSON.stringify(v)));')
            raw = json.loads(subprocess.check_output(['node', '-e', script], text=True))
            self.assertEqual(M.native_bridge_state(raw), expected)
            self.assertNotIn('SECRET', json.dumps(raw))

class NativeStatusDiagnosticTests(unittest.TestCase):
    def test_only_exact_production_status_errors_are_projected(self):
        import subprocess
        messages = [('config validation failed: saved commit unavailable', 'saved-state-unavailable'),
                    ('proxy status unavailable', 'proxy-status-unavailable'),
                    ('proxy status unavailable SECRET', 'unknown'),
                    (' config validation failed: saved commit unavailable', 'unknown'),
                    ('SECRET', 'unknown')]
        for command in ('proxy_get_status', 'config_get', 'config_save', 'auto_select_status', 'proxy_runtime_apply_status'):
            for message, diagnostic in messages:
                script = ('global.window={__TAURI_INTERNALS__:{invoke:()=>Promise.resolve(' +
                    json.dumps({'success': False, 'error': message}) + ')}};const fn=new Function(' +
                    json.dumps(M.NATIVE_BRIDGE_JS) + ');fn(' + json.dumps(command) +
                    ',{},v=>process.stdout.write(JSON.stringify(v)));')
                value = json.loads(subprocess.check_output(['node', '-e', script], text=True))
                expected = diagnostic if command == 'proxy_get_status' else 'unknown'
                self.assertEqual(value['diagnostic'], expected)
                self.assertEqual(M.native_bridge_state(value, command), 'envelope-rejected')
                self.assertNotIn('SECRET', json.dumps(value))
                driver = M.WebDriver(1); driver.session = 'private'
                with patch.object(driver, '_request', return_value=value):
                    with self.assertRaises(M.NativeRejected) as caught: driver.invoke(command)
                self.assertEqual(M.gui_failure(caught.exception)['nativeDiagnostic'], expected)
        forged = {'schema': M.NATIVE_BRIDGE_SCHEMA, 'state': 'envelope-rejected', 'data': None,
                  'diagnostic': 'saved-state-unavailable'}
        self.assertEqual(M.native_bridge_state(forged, 'config_save'), 'invalid-bridge')


class SelectionStageTests(unittest.TestCase):
    def test_connect_status_failure_is_not_attributed_to_completed_pick(self):
        run = M.CarrierRun.__new__(M.CarrierRun)
        run.result = {}; run.stage = 'gui-selection'
        run.status = Mock(side_effect=ValueError('fixed test'))
        with self.assertRaises(ValueError): run.connect()
        self.assertEqual(run.stage, 'gui-connect')
        self.assertEqual(run.result['connectObservation']['phase'], 'stopped-check')

    def test_full_selection_sequence_has_fixed_current_operation_stages(self):
        run = M.CarrierRun.__new__(M.CarrierRun); run.core = {'pid': 1}
        seen = []
        def observed(name, result=None):
            def call(*args, **kwargs):
                seen.append((name, run.stage)); return result
            return call
        run.pick = observed('pick')
        def connect():
            run.stage = 'gui-connect'; seen.append(('connect', run.stage))
        run.connect = connect
        run.socket = observed('socket', (Mock(), {}))
        run.exchange = observed('exchange', {})
        run.preference = observed('preference')
        run.disconnect = observed('disconnect')
        run.config = observed('config', {'selectedServerId': 'node-b'})
        run.record = observed('record'); run.snapshot = observed('snapshot')
        with patch.object(M, 'wait_for', return_value=True), patch.object(M, 'same_process', return_value=False):
            run.selection_cases()
        self.assertEqual([stage for name, stage in seen if name == 'socket'],
            ['selection-initial-traffic', 'selection-new-traffic', 'selection-restored-traffic'])
        self.assertEqual([stage for name, stage in seen if name == 'pick'],
            ['gui-selection', 'selection-switch-b', 'selection-interrupt-switch', 'selection-restore-b'])
        self.assertIn(('exchange', 'selection-preserved-traffic'), seen)
        self.assertIn(('disconnect', 'selection-off'), seen)
        self.assertIn(('config', 'selection-on-readback'), seen)


class SavedPreferenceTests(unittest.TestCase):
    def files(self, root, value=False):
        home=Path(root)/'home'; directory=home/'config/com.polaris.app/polaris'
        directory.mkdir(parents=True)
        path=directory/'config.json'
        path.write_text(json.dumps({'interruptConnectionsOnSwitch':value,'clashApiSecret':'SECRET'}))
        return home,path

    def test_real_file_proof_is_bounded_and_contains_no_config(self):
        with tempfile.TemporaryDirectory() as tmp:
            home,path=self.files(tmp)
            result=M.read_saved_preference(home,home/'config')
            self.assertFalse(result['value']); self.assertEqual(result['sha256'],M.sha(path))
            self.assertNotIn('SECRET',json.dumps(result)); self.assertEqual(set(result),{'source','value','bytes','sha256'})

    def test_fifo_directory_and_symlink_rejected_without_content_read(self):
        for kind in ('fifo','directory','symlink'):
            with self.subTest(kind=kind), tempfile.TemporaryDirectory() as tmp:
                home,path=self.files(tmp);path.unlink()
                if kind=='fifo': os.mkfifo(path)
                elif kind=='directory':path.mkdir()
                else:path.symlink_to(home/'missing')
                with patch.object(M.os,'read',side_effect=AssertionError('must not read')):
                    with self.assertRaises((ValueError,OSError)):M.read_saved_preference(home,home/'config')
        with tempfile.TemporaryDirectory() as tmp:
            home,path=self.files(tmp);original=path.parent; moved=original.with_name('moved');original.rename(moved);original.symlink_to(moved,target_is_directory=True)
            with self.assertRaises((ValueError,OSError)):M.read_saved_preference(home,home/'config')

    def test_owner_root_size_and_value_fail_closed(self):
        with tempfile.TemporaryDirectory() as tmp:
            home,path=self.files(tmp)
            with patch.object(M.os,'getuid',return_value=os.getuid()+1):
                with self.assertRaises(ValueError):M.read_saved_preference(home,home/'config')
            with self.assertRaises(ValueError):M.read_saved_preference(home,home/'elsewhere')
            path.write_bytes(b'x'*(4*1024*1024+1))
            with patch.object(M.os,'read',side_effect=AssertionError('must not read')):
                with self.assertRaises(ValueError):M.read_saved_preference(home,home/'config')
            path.write_text('{"interruptConnectionsOnSwitch":"true"}')
            with self.assertRaises(ValueError):M.read_saved_preference(home,home/'config')

    def test_replacement_during_read_is_not_proof_of_commit(self):
        with tempfile.TemporaryDirectory() as tmp:
            home,path=self.files(tmp);original_read=os.read;replaced=False
            def read(fd,n):
                nonlocal replaced
                value=original_read(fd,n)
                if not replaced:
                    replacement=path.with_name('new');replacement.write_bytes(path.read_bytes());os.replace(replacement,path);replaced=True
                return value
            with patch.object(M.os,'read',side_effect=read):
                with self.assertRaises(ValueError):M.read_saved_preference(home,home/'config')

    def test_fstat_and_close_errors_do_not_leak_fds_or_mask_first_error(self):
        with tempfile.TemporaryDirectory() as tmp:
            home,path=self.files(tmp);close=os.close
            with patch.object(M.os,'fstat',side_effect=OSError('fixed')),patch.object(M.os,'close',wraps=close) as closed:
                with self.assertRaises(OSError):M.read_saved_preference(home,home/'config')
                self.assertEqual(closed.call_count,1)
            def bad_close(fd):close(fd);raise OSError('fixed')
            with patch.object(M.os,'close',side_effect=bad_close) as closed:
                with self.assertRaisesRegex(ValueError,'descriptor close'):M.read_saved_preference(home,home/'config')
                self.assertEqual(closed.call_count,5)
            path.write_bytes(b'bad json')
            with patch.object(M.os,'close',side_effect=bad_close) as closed:
                with self.assertRaises(json.JSONDecodeError):M.read_saved_preference(home,home/'config')
                self.assertEqual(closed.call_count,5)

    def fixture(self, current=False):
        run=M.CarrierRun.__new__(M.CarrierRun);run.result={};run.driver=Mock();run.text=lambda x:x
        run.config=Mock(side_effect=AssertionError('no config IPC'))
        run.preference_pending_save=Mock(side_effect=[False,True,False] if not current else [False,False])
        run.preference_disk=Mock(side_effect=[{'value':current},{'value':True}])
        run.driver.wait_control.side_effect=[{'checked':'true' if current else 'false'},{'checked':'true'}]
        self.row={'sequence':1,'source':'original-config-save-callback','success':True,'status':'saved','versionValid':True,'preference':True}
        run.driver.observations.side_effect=[[],[self.row]]
        return run

    def test_already_wanted_still_requires_disk_clean_and_no_click(self):
        run=self.fixture(True);run.preference(True)
        run.driver.click_control.assert_not_called();run.driver.observations.assert_not_called()
        self.assertEqual(run.preference_disk.call_count,2);run.config.assert_not_called()
        self.assertTrue(run.result['preferenceObservations'][0]['clean'])

    def test_dirty_or_ui_disk_disagreement_never_toggles(self):
        for dirty in (True,False):
            run=self.fixture();run.preference_pending_save=Mock(return_value=dirty)
            if not dirty:run.preference_disk=Mock(return_value={'value':True})
            with self.assertRaises(M.GuiFailure):run.preference(True)
            run.driver.click_control.assert_not_called();run.config.assert_not_called()

    def test_bad_save_callback_never_replays_mutation(self):
        for change in ({'success':False},{'status':'conflict'},{'versionValid':False},{'preference':False},{'extra':'SECRET'}):
            run=self.fixture();run.driver.observations.side_effect=[[],[{**self.row,**change}]]
            with self.subTest(change=change),self.assertRaises(M.GuiFailure):run.preference(True)
            self.assertEqual(run.driver.click_control.call_count,1)
            saves=[c for c in run.driver.click.call_args_list if c.args[0]=='.pending-bar .pb-acts button']
            self.assertEqual(len(saves),1);run.config.assert_not_called()
            self.assertNotIn('SECRET',json.dumps(run.result))
        run=self.fixture();run.driver.observations.side_effect=[[],[self.row,{**self.row,'sequence':2}]]
        with self.assertRaises(M.GuiFailure):run.preference(True)

    def test_saved_callback_does_not_replace_disk_or_app_identity(self):
        run=self.fixture();run.preference_disk.side_effect=[{'value':False},{'value':False}]
        with self.assertRaises(M.GuiFailure):run.preference(True)
        self.assertEqual(run.driver.click_control.call_count,1)
        run=M.CarrierRun.__new__(M.CarrierRun);run.app={'pid':10,'startTime':'1','ppid':2,'state':'S','uid':os.getuid(),'exeDev':1,'exeInode':2}
        run.home=Path('/private');run.env={'XDG_CONFIG_HOME':'/private/config'}
        with patch.object(M,'process_identity',return_value={'pid':1,'startTime':'2'}),patch.object(M,'read_saved_preference') as read:
            with self.assertRaises(M.GuiFailure):run.preference_disk()
            read.assert_not_called()

    def test_confirmation_budget_is_not_reset_or_success_after_expiry(self):
        run=self.fixture(); now=[0.0]; calls=[0]
        def observations():
            calls[0]+=1
            if calls[0]==1:return []
            now[0]=31.0
            return [self.row]
        run.driver.observations.side_effect=observations
        with patch.object(M.time,'monotonic',side_effect=lambda:now[0]):
            with self.assertRaises(M.GuiFailure) as caught:run.preference(True)
        self.assertEqual(caught.exception.code,'observation-timeout')
        self.assertEqual(run.driver.click_control.call_count,1)
        self.assertNotIn('clean',run.result['preferenceObservations'][0])

    def test_app_generation_change_after_disk_read_is_rejected(self):
        run=M.CarrierRun.__new__(M.CarrierRun)
        run.app={'pid':10,'startTime':'1','ppid':2,'state':'S','uid':os.getuid(),'exeDev':1,'exeInode':2}
        run.home=Path('/private');run.env={'XDG_CONFIG_HOME':'/private/config'}
        live={k:run.app[k] for k in ('pid','startTime','ppid','state','uid')}
        image=SimpleNamespace(st_mode=0o100755,st_dev=1,st_ino=2,st_size=10,st_mtime_ns=1,st_ctime_ns=1)
        with patch.object(M,'process_identity',side_effect=[live,live,live,{**live,'startTime':'2'}]), patch.object(M.Path,'stat',return_value=image), patch.object(M,'read_saved_preference',return_value={'value':True}) as read:
            with self.assertRaises(M.GuiFailure):run.preference_disk()
            read.assert_called_once()

    def test_real_observer_projects_save_without_secret_or_command_claim(self):
        import subprocess
        raw={'success':True,'data':{'status':'saved','version':'1234abcd','config':{'interruptConnectionsOnSwitch':True,'secret':'SECRET'}}}
        script=('const callbacks=new Map([[1,x=>{}]]);global.window={};const internals={};'
            'Object.defineProperty(internals,"invoke",{value:()=>{},writable:false,configurable:false});'
            'Object.defineProperty(internals,"callbacks",{value:callbacks,writable:false,configurable:false});'
            'window.__TAURI_INTERNALS__=internals;new Function('+json.dumps(M.OBSERVER)+')();'
            'callbacks.get(1)('+json.dumps(raw)+');process.stdout.write(JSON.stringify(window.__packageObservation.rows));')
        rows=json.loads(subprocess.check_output(['node','-e',script],text=True))
        self.assertTrue(rows[0]['versionValid']);self.assertTrue(rows[0]['preference'])
        self.assertNotIn('SECRET',json.dumps(rows));self.assertNotIn('command',rows[0])
        source=Path(M.__file__).read_text();body=source.split('    def preference(self, wanted):',1)[1].split('    def socket(',1)[0]
        self.assertNotIn('self.config(',body);self.assertNotIn('invoke(',body)
        self.assertIn('wait_for(closed, timeout=12)',source)


class DriverWireTests(unittest.TestCase):
    def fixture(self, mode):
        import contextlib
        import time
        import threading
        from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
        @contextlib.contextmanager
        def serve():
            rows=[]; closed=threading.Event()
            class Handler(BaseHTTPRequestHandler):
                protocol_version='HTTP/1.1'
                def log_message(self,*args): pass
                def do_POST(self):
                    body=self.rfile.read(int(self.headers.get('Content-Length','0')))
                    rows.append({'headers':dict(self.headers),'body':body,'path':self.path})
                    if mode=='eof': self.close_connection=True; return
                    if mode=='timeout': time.sleep(.3);self.close_connection=True;return
                    code=200; value={'ok':True,'padding':'x'*100000}
                    if mode in ('http','w3c'): value={'error':'invalid session id','message':'PRIVATE'}
                    if mode in ('http','huge-error'):code=404
                    if mode=='redirect':code=302
                    raw=json.dumps({'value':value}).encode() if mode!='json' else b'not-json'
                    self.send_response(code)
                    if mode in ('chunked','chunked-incomplete'):
                        self.send_header('Transfer-Encoding','chunked')
                    else:
                        self.send_header('Content-Length',str(len(raw)+(10 if mode=='valid-json-truncated' else 0)))
                    if mode=='close':self.send_header('Connection','close')
                    if mode=='redirect':self.send_header('Location','http://127.0.0.1:1/private')
                    self.end_headers()
                    try:
                        if mode in ('chunked','chunked-incomplete'):
                            self.wfile.write(('%x\r\n'%len(raw)).encode()+raw+b'\r\n')
                            if mode=='chunked':self.wfile.write(b'0\r\n\r\n')
                            else:self.close_connection=True
                            self.wfile.flush();return
                        if mode=='valid-json-truncated':
                            self.wfile.write(raw);self.wfile.flush();self.close_connection=True;return
                        if mode=='partial':
                            self.wfile.write(raw[:5]);self.wfile.flush();self.close_connection=True;return
                        if mode=='slow':
                            for chunk in (raw[:1],raw[1:2],raw[2:3],raw[3:]):
                                self.wfile.write(chunk);self.wfile.flush();time.sleep(.06)
                        else:self.wfile.write(raw);self.wfile.flush()
                    except (BrokenPipeError,ConnectionResetError):pass
                def finish(self):
                    try:super().finish()
                    finally:closed.set()
            server=ThreadingHTTPServer(('127.0.0.1',0),Handler)
            thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
            try:yield server.server_port,rows,closed
            finally:server.shutdown();server.server_close();thread.join(2)
        return serve()

    def test_real_wire_success_and_server_close_consume_full_body_without_hop_header(self):
        for mode in ('success','close','chunked'):
            with self.subTest(mode=mode),self.fixture(mode) as (port,rows,closed):
                driver=M.WebDriver(port)
                value=driver.request('POST','/session/fixed/execute/sync',{'fixture':True},timeout=1)
                self.assertTrue(value['ok']);self.assertEqual(len(value['padding']),100000)
                self.assertTrue(closed.wait(1));self.assertEqual(len(rows),1)
                self.assertNotIn('connection',{k.lower() for k in rows[0]['headers']})
                self.assertEqual(json.loads(rows[0]['body']),{'fixture':True})

    def test_real_wire_errors_fail_once_without_replay_or_redirect(self):
        for mode,error in [('eof',M.http.client.RemoteDisconnected),('timeout',TimeoutError),
            ('slow',TimeoutError),('http',M.GuiFailure),('huge-error',M.GuiFailure),('w3c',M.GuiFailure),
            ('redirect',M.GuiFailure),('json',json.JSONDecodeError),('partial',M.http.client.IncompleteRead),
            ('valid-json-truncated',M.http.client.IncompleteRead),('chunked-incomplete',M.http.client.IncompleteRead)]:
            with self.subTest(mode=mode),self.fixture(mode) as (port,rows,closed):
                driver=M.WebDriver(port);driver.session='fixed'
                with self.assertRaises(error):
                    driver.request('POST',driver.path('/execute/async'),{'fixture':True},timeout=.12)
                self.assertTrue(closed.wait(1));self.assertEqual(len(rows),1)
                self.assertNotIn('connection',{k.lower() for k in rows[0]['headers']})
                self.assertEqual(driver.observation['firstFailure']['state'],'failed')
                self.assertNotIn('PRIVATE',json.dumps(driver.observation))

    def test_error_body_bound_closes_without_reading_remaining_bytes(self):
        driver=M.WebDriver(1);response=Mock(status=500,length=999999)
        response.isclosed.return_value=False
        response.read1.return_value=b'x'*16385
        connection=Mock();connection.getresponse.return_value=response
        with patch.object(M.http.client,'HTTPConnection',return_value=connection):
            with self.assertRaises(M.GuiFailure) as error:driver.request('POST','/fixed',{})
        response.read1.assert_called_once_with(16385)
        response.close.assert_called_once();connection.close.assert_called_once()
        self.assertEqual(M.gui_failure(error.exception),{'code':'webdriver-http-error','httpStatus':500,'protocol':'unknown'})

    def test_invalid_budget_never_connects(self):
        for timeout in (0,-1,True,float('inf'),float('nan'),'25'):
            with self.subTest(timeout=timeout),patch.object(M.http.client,'HTTPConnection') as create:
                with self.assertRaises(ValueError):M.WebDriver(1)._request('POST','/fixed',{},timeout)
                create.assert_not_called()

    def test_header_return_over_budget_is_failed_and_all_handles_closed(self):
        driver=M.WebDriver(1);response=Mock(status=200,length=0)
        connection=Mock();connection.getresponse.return_value=response
        with patch.object(M.http.client,'HTTPConnection',return_value=connection), \
             patch.object(M.time,'monotonic',side_effect=[0,0,0,26]):
            with self.assertRaises(TimeoutError):driver._request('POST','/fixed',{},timeout=25)
        connection.request.assert_called_once();response.close.assert_called_once();connection.close.assert_called_once()

    def test_http_connection_is_fresh_for_every_mutation_and_read(self):
        for command in ('config_get','config_save','proxy_get_status'):
            driver=M.WebDriver(1);driver.session='fixed';conn=Mock()
            conn.getresponse.side_effect=M.http.client.RemoteDisconnected()
            with patch.object(M.http.client,'HTTPConnection',return_value=conn) as create:
                with self.assertRaises(M.http.client.RemoteDisconnected):driver.invoke(command)
            create.assert_called_once();conn.request.assert_called_once();conn.close.assert_called_once()


class AppIdentityProducerTests(unittest.TestCase):
    def child(self):
        import contextlib
        import subprocess
        @contextlib.contextmanager
        def running():
            child=subprocess.Popen([sys.executable,'-c','import sys; sys.stdin.read()'],stdin=subprocess.PIPE)
            try:
                raw=M.process_identity(child.pid)
                supervisor=M.Supervisor.__new__(M.Supervisor)
                supervisor.living=lambda:[raw]
                supervisor.mark_product=Mock()
                app=supervisor.executable(Path(sys.executable).resolve())
                yield raw,app
            finally:
                child.communicate(timeout=5)
        return running()

    def test_real_producer_capture_and_preference_read_without_fabricated_exe_fields(self):
        with self.child() as (raw,app), tempfile.TemporaryDirectory() as tmp:
            self.assertEqual(set(raw),{'pid','startTime','ppid','state','uid'})
            self.assertEqual(set(app),set(raw)|{'sha256','exe'})
            captured=M.app_execution_identity(app)
            self.assertEqual(set(captured),set(app)|{'exeDev','exeInode'})
            home=Path(tmp);path=home/'config/com.polaris.app/polaris/config.json'
            path.parent.mkdir(parents=True);path.write_text(json.dumps({'interruptConnectionsOnSwitch':True}))
            run=M.CarrierRun.__new__(M.CarrierRun);run.app=captured;run.home=home
            run.env={'XDG_CONFIG_HOME':str(home/'config')}
            self.assertTrue(run.preference_disk()['value'])
            self.assertNotIn('exeDev',M.process_identity(app['pid']))

    def test_invalid_identity_container_is_classified_before_copy_or_proc_read(self):
        for value in (None, [], 'private', 10):
            run=M.CarrierRun.__new__(M.CarrierRun);run.app=value
            with patch.object(M,'process_identity') as proc,self.assertRaises(M.GuiFailure) as error:
                run.preference_disk()
            self.assertEqual(error.exception.code,'app-identity-schema-invalid');proc.assert_not_called()

    def test_each_producer_field_missing_or_wrong_type_is_schema_failure(self):
        with self.child() as (raw,app):
            for field in ('pid','startTime','ppid','uid','state','sha256','exe'):
                for missing in (True,False):
                    bad=dict(app)
                    if missing:bad.pop(field)
                    else:bad[field]=False if field in ('pid','ppid','uid') else 123
                    with self.subTest(field=field,missing=missing),self.assertRaises(M.GuiFailure) as e:
                        M.app_execution_identity(bad)
                    self.assertEqual(e.exception.code,'app-identity-schema-invalid')
            captured=M.app_execution_identity(app)
            for field in ('exeDev','exeInode'):
                for invalid in (None,False,'1'):
                    bad={**captured,field:invalid}
                    with self.subTest(field=field,value=invalid),self.assertRaises(M.GuiFailure) as e:
                        M.validate_app_identity(bad,captured=True)
                    self.assertEqual(e.exception.code,'app-identity-schema-invalid')
            for field in raw:
                incomplete=dict(raw);incomplete.pop(field)
                with patch.object(M,'process_identity',return_value=incomplete),self.assertRaises(M.GuiFailure) as e:
                    M.observe_app_executable(app)
                self.assertEqual(e.exception.code,'app-identity-schema-invalid')

    def test_real_producer_changed_birth_uid_hash_and_executable_fail(self):
        with self.child() as (raw,app):
            for change in ({'startTime':str(int(raw['startTime'])+1)}, {'uid':raw['uid']+1}, {'state':'Z'}):
                with patch.object(M,'process_identity',return_value={**raw,**change}),self.assertRaises(M.GuiFailure) as e:
                    M.observe_app_executable(app)
                self.assertEqual(e.exception.code,'app-identity-changed')
            with self.assertRaises(M.GuiFailure) as e:M.app_execution_identity({**app,'sha256':'0'*64})
            self.assertEqual(e.exception.code,'app-identity-changed')
            with patch.object(M.Path,'stat',side_effect=PermissionError(13,'PRIVATE')),self.assertRaises(M.GuiFailure) as e:
                M.observe_app_executable(app)
            self.assertEqual(e.exception.code,'app-executable-unreadable');self.assertNotIn('PRIVATE',str(e.exception))
            captured=M.app_execution_identity(app)
            run=M.CarrierRun.__new__(M.CarrierRun);run.app={**captured,'exeInode':captured['exeInode']+1}
            with patch.object(M,'read_saved_preference') as read,self.assertRaises(M.GuiFailure) as e:run.preference_disk()
            self.assertEqual(e.exception.code,'preference-app-identity-changed');read.assert_not_called()

    def test_fresh_stat_fences_birth_on_both_sides_and_capture_rechecks_inode(self):
        with self.child() as (raw,app):
            with patch.object(M,'process_identity',side_effect=[raw,{**raw,'startTime':str(int(raw['startTime'])+1)},raw]),self.assertRaises(M.GuiFailure) as e:
                M.observe_app_executable(app)
            self.assertEqual(e.exception.code,'app-identity-changed')
            captured=M.app_execution_identity(app)
            with self.assertRaises(M.GuiFailure) as e:M.app_execution_identity({**captured,'exeInode':captured['exeInode']+1})
            self.assertEqual(e.exception.code,'app-identity-changed')

    def test_same_birth_exec_between_metadata_samples_is_rejected(self):
        with self.child() as (raw,app):
            info=(Path('/proc')/str(app['pid'])/'exe').stat()
            fields=('st_dev','st_ino','st_size','st_mtime_ns','st_ctime_ns','st_mode')
            changed=SimpleNamespace(**{k:getattr(info,k)+(1 if k=='st_ino' else 0) for k in fields})
            with patch.object(M,'process_identity',return_value=raw),patch.object(M.Path,'stat',side_effect=[info,changed]),self.assertRaises(M.GuiFailure) as error:
                M.observe_app_executable(app)
            self.assertEqual(error.exception.code,'app-identity-changed')

    def test_shared_capture_keeps_plain_and_tun_identity_sources_distinct(self):
        shared=Path(M.__file__).read_text();tun=Path(M.__file__).with_name('linux_tun.py').read_text()
        self.assertIn('self.app = app_execution_identity(app)',shared)
        body=tun.split('def app_execution_identity(app):',1)[1].split('def verify_gui_binding',1)[0]
        self.assertIn('return base.app_execution_identity(app)',body)
        self.assertNotIn('tun.process_identity',body.split('return',1)[1])


if __name__ == '__main__':
    unittest.main()
