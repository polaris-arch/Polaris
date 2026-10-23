#!/usr/bin/env python3
"""Pure/mocked TUN GUI contracts. Never starts product/network/root/tools."""
import ast
import copy
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import sys
import tempfile
import threading
import unittest
from unittest.mock import Mock, patch
from types import SimpleNamespace

HERE = Path(__file__).resolve().parent
SUPPORT = Path(os.environ.get('POLARIS_TUN_SUPPORT_DIR', HERE))
BASE = Path(os.environ.get('POLARIS_ACCEPTANCE_TEST_SOURCE_ROOT', HERE.parents[1])) / 'scripts/package-acceptance'
TEMP = tempfile.TemporaryDirectory(prefix='polaris-tun-gui-unit-')
for name, directory in (('linux_tun.py', HERE), ('linux.py', HERE), ('tun.py', SUPPORT),
                        ('tun_fixtures.py', HERE if (HERE / 'tun_fixtures.py').exists() else SUPPORT), ('fixtures.py', BASE)):
    shutil.copyfile(directory / name, Path(TEMP.name) / name)
sys.path.insert(0, TEMP.name)
spec = importlib.util.spec_from_file_location('tested_linux_tun', Path(TEMP.name) / 'linux_tun.py')
M = importlib.util.module_from_spec(spec); spec.loader.exec_module(M)
sys.path.remove(TEMP.name)
H = 'a' * 64


def identity(pid=10, uid=1001, parent=9):
    return {'pid': pid, 'startTime': '1234', 'state': 'S', 'ppid': parent, 'uid': uid,
            'exeDev': 1, 'exeInode': pid + 100, 'exe': '/usr/bin/sing-box',
            'namespaces': {'net': 5, 'mnt': 6, 'pid': 7, 'cgroup': 8}, 'cgroup': '0::/test.slice'}


def supervisor():
    obj = M.TunSupervisor.__new__(M.TunSupervisor)
    obj.lock = threading.RLock(); obj.window = None; obj.quarantine = {}; obj.confirmed = set()
    obj.controller_birth = None; obj.controller_descendants = {}; obj.gui_birth = None
    obj.owned = {}; obj.children = {}; obj.roles = {}; obj.unknown = []; obj.forced = []
    return obj


def fixture_evidence():
    plan = M.tun.topology('123456789abc')
    streams = {label: {'exit': label, 'bytes': 7, 'nonceSha256': (label.lower() * 64)} for label in ('A', 'B')}
    dns = [{'transport': mode, 'destination': '8.8.8.8', 'name': mode + '.fixture.invalid', 'type': 1,
            'answer': plan['origin'], 'querySha256': letter * 64} for mode, letter in (('udp', 'c'), ('tcp', 'd'))]
    analysis = {'analysisExecution': 'real-tool', 'captureSha256': H, 'packets': 8,
                'nonces': list(streams.values()), 'dnsQueries': copy.deepcopy(dns)}
    snapshots = {role: {'role': role, 'errors': [], 'rows': []} for role in ('A', 'B', 'O')}
    snapshots['O']['rows'] = [{**r, 'kind': 'echo', 'originPeer': plan['exit'][r['exit']]} for r in streams.values()]
    snapshots['A']['rows'] = [{**r, 'ttl': 0} for r in dns]
    return plan, analysis, snapshots, streams, dns


class PureContracts(unittest.TestCase):
    def test_source_compiles_without_import_actions(self):
        source = (HERE / 'linux_tun.py').read_text()
        tree = ast.parse(source)
        self.assertNotIn('authenticated=true', source)
        self.assertFalse(any(isinstance(node, ast.Expr) and isinstance(node.value, ast.Call)
                             for node in tree.body))
        self.assertIn('not six-RPC TUN acceptance', source)

    def test_non_tun_default_unchanged(self):
        source = (HERE / 'linux.py').read_text()
        self.assertIn("require(status.get('startedViaHelper') is not True", source)
        self.assertIn("all(item.get('type') != 'tun'", source)
        self.assertIn("require(item['uid'] == os.getuid(), 'owned descendant elevated unexpectedly')", source)
        self.assertIn('return Supervisor()', source)
        self.assertIn('return self.fixtures.Network()', source)

    def test_runtime_scope_and_no_direct_main_action_ipc(self):
        source = (HERE / 'linux_tun.py').read_text()
        for command in ('helper_install', 'helper_uninstall', 'proxy_start', 'proxy_stop', 'server_switch', 'tray_quit'):
            self.assertNotIn("invoke('" + command, source)
        self.assertIn("self.driver.click('#connect-btn')", source)
        self.assertIn('self.driver.quit_shortcut()', source)
        self.assertIn('self.helper_action(\'uninstall\')', source)
        self.assertIn("self.controller.call('product_uninstalled')", source)
        self.assertNotIn('dpkg --install', source)

    def test_observer_preserves_original_callbacks(self):
        self.assertIn('Reflect.apply(prior,this,[id])', M.TUN_OBSERVER)
        self.assertIn('Reflect.apply(callback,this,args)', M.TUN_OBSERVER)
        self.assertNotIn('.invoke=', M.TUN_OBSERVER)
        self.assertNotIn('secret', M.TUN_OBSERVER)
        self.assertIn('state.rows.length>256', M.TUN_OBSERVER)

    def inspection_fixture(self, root):
        directory = root / 'usr/lib/Polaris/_up_/resources/linux'
        directory.mkdir(parents=True)
        for name in ('sing-box', 'libcronet.so'):
            (directory / name).write_bytes(b'fixture-not-executable')
        helper = directory / 'polaris-helper'
        helper.write_bytes(b'\x7fELFfixture'); helper.chmod(0o755)
        manifest = directory / 'core-manifest.json'
        manifest.write_text('{"schema":"polaris-core-manifest-v1","id":"mock-only"}')
        upper = root / 'usr/lib/Polaris/core-manifest.json'
        upper.write_text('{"bundledCoreVersion":"1.15.0-alpha.8","desktopRelease":{}}')
        members = {role: M.base.byte_identity(directory / name, root)
                   for role, name in (('core', 'sing-box'), ('cronet', 'libcronet.so'))}
        return directory, helper, manifest, upper, members

    def test_real_dual_role_layout_selects_only_frozen_core_neighbor(self):
        # Real carrier path topology, synthetic bytes: not runtime evidence.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            directory, helper, manifest, upper, members = self.inspection_fixture(root)
            result = M.inspect_helper(root, members)
            self.assertEqual(result['manifest'], M.base.byte_identity(manifest, root))
            self.assertEqual(result['helper'], M.base.byte_identity(helper, root))
            self.assertEqual(result['core'], members['core'])
            self.assertEqual(result['cronet'], members['cronet'])
            self.assertEqual(result['embeddedManifest']['id'], 'mock-only')
            self.assertTrue(upper.exists())
            self.assertEqual(len(list(root.rglob('core-manifest.json'))), 2)

    def test_adjacent_manifest_missing_linked_invalid_or_oversized_never_falls_back(self):
        for fault in ('missing', 'symlink', 'invalid-json', 'oversize', 'directory'):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                directory, helper, manifest, upper, members = self.inspection_fixture(root)
                manifest.unlink()
                if fault == 'symlink': manifest.symlink_to(upper)
                elif fault == 'invalid-json': manifest.write_text('{')
                elif fault == 'oversize': manifest.write_text(' ' * 65537)
                elif fault == 'directory': manifest.mkdir()
                with self.assertRaises(ValueError): M.inspect_helper(root, members)

    def test_exact_helper_elf_mode_link_and_uniqueness_fail_closed(self):
        for fault in ('missing', 'foreign-only', 'symlink', 'not-elf', 'not-executable', 'duplicate'):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                directory, helper, manifest, upper, members = self.inspection_fixture(root)
                if fault in ('missing', 'foreign-only', 'symlink'): helper.unlink()
                if fault in ('foreign-only', 'symlink', 'duplicate'):
                    other = root / 'polaris-helper'; other.write_bytes(b'\x7fELF'); other.chmod(0o755)
                    if fault == 'symlink': helper.symlink_to(other)
                elif fault == 'not-elf': helper.write_bytes(b'not-ELF')
                elif fault == 'not-executable': helper.chmod(0o644)
                with self.assertRaises(ValueError): M.inspect_helper(root, members)

    def test_frozen_member_paths_must_be_regular_adjacent_and_inside_payload(self):
        for fault in ('split', 'core-link', 'ancestor-link', 'escape'):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                directory, helper, manifest, upper, members = self.inspection_fixture(root)
                if fault == 'split': members['cronet']['path'] = str(upper.relative_to(root))
                elif fault == 'core-link':
                    core = directory / 'sing-box'; core.unlink(); core.symlink_to(upper)
                elif fault == 'ancestor-link':
                    alias = root / 'alias'; alias.symlink_to(directory, target_is_directory=True)
                    members['core']['path'] = 'alias/sing-box'
                else: members['core']['path'] = str(Path(__file__).absolute())
                with self.assertRaises(ValueError): M.inspect_helper(root, members)

    def test_entry_diagnostics_are_fixed_reached_stages_not_secret_messages_or_pass(self):
        for fault in ('inspect-helper', 'bind-helper', 'authorize', 'runtime',
                      'post-runtime-carrier', 'binding-copy', 'returned-runtime-failure',
                      'construct', 'workspace-mounted', 'workspace-error', 'ordinary-close-error',
                      'workspace-after-success', 'processes-stopped', 'controller-unconfirmed', 'controller-reaped',
                      'runtime-second-failure'):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp); package = root / 'package'; package.write_bytes(b'package')
                transport = root / 'transport'; transport.write_bytes(b'transport')
                rpc = root / 'rpc'; rpc.write_bytes(b'not-executed')
                admission = {'schema': 'polaris-linux-tun-admission-v1',
                    'kernelCommit': M.tun.KERNEL_COMMIT, 'sourceScope': 'same-final-carrier',
                    'publicationEligible': False, 'transportSha256': M.base.sha(transport),
                    'carrier': {'kind': 'deb', 'sha256': M.base.sha(package)}, 'context': {}}
                admitted = root / 'admitted.json'; admitted.write_text(json.dumps(admission))
                args = SimpleNamespace(admission=admitted, transport=transport, packages=root,
                    output=root / 'run.json', rpc_client=rpc, kind='deb')
                run = Mock(); run.run_id = '123456789abc'; run.controller = None
                run.supervisor.close.return_value = {'state': 'failed', 'unknown': ['unconfirmed']}
                secret = 'PRIVATE-EXCEPTION-CONTENT-NEVER-EXPORT'
                if fault == 'ordinary-close-error': run.supervisor.close.side_effect = PermissionError(secret)
                if fault in ('controller-unconfirmed', 'controller-reaped'):
                    run.controller = SimpleNamespace(closed=fault == 'controller-reaped')
                cleanup_authorizations = []
                real_cleanup = M.base.cleanup_workspace
                def workspace_cleanup(directory, *, processes_stopped=False):
                    cleanup_authorizations.append(processes_stopped)
                    # Do not leak mock workspaces: no process was started in this test.
                    real_cleanup(directory, processes_stopped=True)
                    value = {'state': 'normal'} if processes_stopped else {
                        'state': 'failed', 'reason': 'workspace-process-exit-unconfirmed'}
                    if fault in ('workspace-mounted', 'workspace-error', 'workspace-after-success', 'processes-stopped', 'controller-unconfirmed', 'controller-reaped',
                      'runtime-second-failure'):
                        return {'state': 'failed', 'reason': 'workspace-mount-remains' if fault == 'workspace-mounted'
                                else 'workspace-cleanup-unconfirmed'}
                    return value
                def node(argv, *unused, **kwargs):
                    if argv[0] == fault: raise ValueError(secret)
                    if argv[0] == 'bind-helper':
                        Path(argv[3]).write_text(json.dumps({'schema': 'polaris-linux-tun-bound-v1',
                                                          'admission': admission}))
                    return {}
                def runtime():
                    if fault in ('runtime', 'ordinary-close-error'): raise ValueError(secret)
                    if fault == 'post-runtime-carrier': package.write_bytes(b'changed')
                    if fault == 'binding-copy': (root / 'deb-bound.json').write_text('occupied')
                    result = M.tun.report_result({}, {'state': 'failed', 'forced': False,
                        'unknown': ['runtime-cleanup-unconfirmed']}, 'same-final-carrier')
                    if fault == 'workspace-after-success':
                        # Synthetic boundary response only; cleanup must veto even a success label.
                        return {'result': 'success', 'cases': {}, 'blocked': [],
                                'cleanup': {'state': 'normal', 'forced': False, 'unknown': []}}
                    if fault in ('processes-stopped', 'controller-unconfirmed', 'controller-reaped'):
                        result['ordinaryCleanup'] = {'state': 'normal', 'remainingAssessment': 'observed',
                                                     'remaining': [], 'unknown': []}
                        if fault == 'controller-reaped': result['cleanup']['privilegedProcessesReaped'] = True
                    result['rootCleanupEvidence'] = {'marker': 'mock-prior-evidence'}
                    result['failure'] = {'stage': 'native-launch', 'kind': 'ValueError',
                                         'code': 'TUN_RUNTIME_FAILED'}
                    if fault == 'runtime-second-failure':
                        result['ordinaryCleanup'] = {'state': 'normal', 'remainingAssessment': 'observed',
                                                     'remaining': [], 'unknown': []}
                        result['cases'] = {'mock-original-case': {'status': 'failed', 'evidence': {}}}
                        run.result = result
                        raise ValueError(secret)
                    return result
                run.run.side_effect = runtime
                with patch.object(M.base, 'carrier_path', return_value=package), \
                     patch.object(M, 'TunCarrierRun', return_value=run,
                                  side_effect=ValueError(secret) if fault == 'construct' else None), \
                     patch.object(M, 'inspect_helper', side_effect=ValueError(secret) if fault == 'inspect-helper' else None,
                                  return_value={}), patch.object(M, 'run_node', side_effect=node), \
                     patch.object(M.base, 'cleanup_workspace', side_effect=workspace_cleanup):
                    result = M.run_all(args)
                if fault == 'workspace-after-success':
                    self.assertEqual(result['entryProgress']['lastReached'], 'binding-copy')
                    self.assertEqual(result['workspaceCleanup']['state'], 'failed')
                    self.assertEqual(result['cleanup']['state'], 'failed')
                    self.assertIn('workspace-cleanup-unconfirmed', result['cleanup']['unknown'])
                elif fault in ('returned-runtime-failure', 'workspace-mounted', 'workspace-error',
                                'processes-stopped', 'controller-unconfirmed', 'controller-reaped'):
                    self.assertEqual(result['failure']['stage'], 'native-launch')
                    self.assertEqual(result['failure']['code'], 'TUN_RUNTIME_FAILED')
                    self.assertEqual(result['entryProgress']['lastReached'], 'binding-copy')
                    self.assertIn('runtime-cleanup-unconfirmed', result['cleanup']['unknown'])
                    if fault.startswith('workspace-'):
                        self.assertIn('workspace-cleanup-unconfirmed', result['cleanup']['unknown'])
                        self.assertEqual(result['workspaceCleanup']['state'], 'failed')
                else:
                    expected_stage = 'runtime' if fault in ('ordinary-close-error', 'runtime-second-failure') else fault
                    self.assertEqual(result['entryFailure']['stage'], expected_stage)
                    self.assertEqual(result['entryFailure']['code'], M.ENTRY_FAILURE_CODES[expected_stage])
                    if fault in ('post-runtime-carrier', 'binding-copy', 'runtime-second-failure'):
                        self.assertEqual(result['failure']['stage'], 'native-launch')
                        self.assertIn('runtime-cleanup-unconfirmed', result['cleanup']['unknown'])
                        self.assertEqual(result['rootCleanupEvidence'], {'marker': 'mock-prior-evidence'})
                        if fault == 'runtime-second-failure':
                            self.assertIn('mock-original-case', result['cases'])
                            self.assertEqual(result['ordinaryCleanup']['state'], 'normal')
                            self.assertEqual(result['boundaryOrdinaryCleanup']['state'], 'failed')
                    else:
                        self.assertEqual(result['failure'], result['entryFailure'])
                    self.assertEqual(result['entryProgress']['lastReached'], expected_stage)
                    if fault == 'ordinary-close-error':
                        self.assertIn('ordinary-cleanup-unconfirmed', result['ordinaryCleanup']['unknown'])
                    self.assertIn('admission-or-authorization-unconfirmed', result['cleanup']['unknown'])
                self.assertEqual(cleanup_authorizations, [fault in ('processes-stopped', 'controller-reaped')])
                self.assertEqual(result['entryProgress']['claim'], 'entry-reached-only-not-completion')
                self.assertEqual(result['result'], 'failed')
                self.assertNotIn(secret, args.output.read_text())
                self.assertEqual(json.loads(args.output.read_text()), result)
                if fault in ('inspect-helper', 'bind-helper', 'authorize'): run.run.assert_not_called()

    def test_real_capture_origin_dns_join(self):
        plan, analysis, snapshots, streams, dns = fixture_evidence()
        rows, queries = M.join_capture(analysis, snapshots, streams, dns, plan)
        self.assertEqual(set(rows), {'A', 'B'})
        self.assertEqual({r['transport'] for r in queries}, {'udp', 'tcp'})
        M.tun.validate_case('tun_data_plane', {**rows, 'dns': queries})

    def test_simulated_capture_never_runtime_pass(self):
        plan, analysis, snapshots, streams, dns = fixture_evidence()
        analysis['analysisExecution'] = 'simulated'
        with self.assertRaises(ValueError): M.join_capture(analysis, snapshots, streams, dns, plan)

    def test_missing_origin_capture_nonce_or_duplicate_fails(self):
        for fault in ('origin', 'capture', 'duplicate'):
            plan, analysis, snapshots, streams, dns = fixture_evidence()
            if fault == 'origin': snapshots['O']['rows'].pop()
            elif fault == 'capture': analysis['nonces'].pop()
            else: snapshots['O']['rows'].append(copy.deepcopy(snapshots['O']['rows'][0]))
            with self.assertRaises(ValueError): M.join_capture(analysis, snapshots, streams, dns, plan)

    def test_wrong_egress_or_byte_count_fails(self):
        for field, value in (('originPeer', '10.231.240.4'), ('bytes', 8)):
            plan, analysis, snapshots, streams, dns = fixture_evidence()
            snapshots['O']['rows'][0][field] = value
            with self.assertRaises(ValueError): M.join_capture(analysis, snapshots, streams, dns, plan)

    def test_dns_client_packet_hash_not_confused_with_backend_hash(self):
        plan, analysis, snapshots, streams, dns = fixture_evidence()
        snapshots['A']['rows'][0]['querySha256'] = 'e' * 64
        _, queries = M.join_capture(analysis, snapshots, streams, dns, plan)
        self.assertNotEqual(queries[0]['querySha256'], queries[0]['backendQuerySha256'])
        analysis['dnsQueries'][0]['querySha256'] = 'f' * 64
        with self.assertRaises(ValueError): M.join_capture(analysis, snapshots, streams, dns, plan)

    def test_dns_ttl_or_missing_backend_fails(self):
        for fault in ('ttl', 'missing', 'errors'):
            plan, analysis, snapshots, streams, dns = fixture_evidence()
            if fault == 'ttl': snapshots['A']['rows'][0]['ttl'] = 1
            elif fault == 'missing': snapshots['A']['rows'] = []
            else: snapshots['B']['errors'] = ['failed']
            with self.assertRaises(ValueError): M.join_capture(analysis, snapshots, streams, dns, plan)

    def test_controller_root_quarantine_not_owned(self):
        obj = supervisor(); gui = identity(10, os.getuid()); root = identity(11, 0, 10)
        obj.owned[10] = gui; obj.roles[10] = 'product'; obj.begin_window('install', gui)
        with patch.object(M.Path, 'iterdir', return_value=[Path('/proc/10'), Path('/proc/11')]), \
             patch.object(M.base, 'process_identity', side_effect=lambda pid: {10: gui, 11: root}.get(pid)):
            obj.scan()
        self.assertNotIn(11, obj.owned)
        self.assertIn((11, '1234'), obj.quarantine)
        obj.end_window([root])
        self.assertIn((11, '1234'), obj.confirmed)

    def test_same_birth_exec_elevation_is_removed_from_owned(self):
        obj = supervisor(); gui = identity(10, os.getuid()); ordinary = identity(11, os.getuid(), 10)
        elevated = {**ordinary, 'uid': 0}
        obj.owned = {10: gui, 11: ordinary}; obj.roles = {10: 'product', 11: 'product'}
        obj.begin_window('install', gui)
        with patch.object(M.Path, 'iterdir', return_value=[Path('/proc/10'), Path('/proc/11')]), \
             patch.object(M.base, 'process_identity', side_effect=lambda pid: {10: gui, 11: elevated}.get(pid)):
            obj.scan()
        self.assertNotIn(11, obj.owned)
        self.assertNotIn(11, obj.roles)
        self.assertIn((11, '1234'), obj.quarantine)
        obj.end_window([elevated])

    def test_missed_root_ancestry_fails_closed(self):
        obj = supervisor(); obj.begin_window('install', identity())
        obj.window['births'].add((11, '1234'))
        with self.assertRaises(ValueError): obj.end_window([identity(12)])
        self.assertIsNotNone(obj.window)

    def test_foreign_outside_transaction_permanent_unknown(self):
        obj = supervisor(); gui = identity(10, os.getuid()); root = identity(11, 0, 10)
        obj.owned[10] = gui; obj.roles[10] = 'product'
        with patch.object(M.Path, 'iterdir', return_value=[Path('/proc/10'), Path('/proc/11')]), \
             patch.object(M.base, 'process_identity', side_effect=lambda pid: {10: gui, 11: root}.get(pid)):
            obj.scan()
        self.assertIn('foreign-descendant-outside-exact-GUI-transaction', obj.unknown)
        self.assertNotIn(11, obj.owned)

    def test_privileged_cleanup_observation_failure_still_enters_shared_close(self):
        obj = supervisor(); root = identity(11, 0)
        obj.quarantine[(11, '1234')] = root
        with patch.object(M.base, 'process_identity', side_effect=PermissionError('private-detail')), \
             patch.object(M.base.Supervisor, 'close', return_value={'state': 'failed'}) as close:
            result = obj.close(False)
        close.assert_called_once_with(False)
        self.assertEqual(result['state'], 'failed')
        self.assertIn('privileged-cleanup-observation-unconfirmed', obj.unknown)
        self.assertNotIn('private-detail', json.dumps(obj.unknown))

    def test_root_signal_prohibited_even_after_confirmation(self):
        obj = supervisor(); root = identity(11, 0); obj.owned[11] = root
        with patch.object(M.base, 'process_identity', return_value=root), \
             patch.object(M.base.Supervisor, 'signal') as sent, self.assertRaises(ValueError):
            obj.signal(root, 15)
        sent.assert_not_called()

    def test_root_controller_birth_not_ordinary_owned(self):
        obj = supervisor(); root = identity(11, 0, os.getpid()); child = identity(12, 0, 11)
        obj.controller_birth = root; obj.controller_descendants[11] = root
        with patch.object(M.Path, 'iterdir', return_value=[Path('/proc/11'), Path('/proc/12')]), \
             patch.object(M.base, 'process_identity', side_effect=lambda pid: {11: root, 12: child}.get(pid)):
            obj.scan()
        self.assertFalse(obj.owned)
        self.assertEqual(set(obj.controller_descendants), {11, 12})

    def test_direct_driver_not_inherited_role_descendants(self):
        obj = supervisor(); direct = identity(10, os.getuid()); child = identity(11, os.getuid(), 10)
        obj.owned = {10: direct, 11: child}; obj.roles = {10: 'driver', 11: 'driver'}
        obj.children = {10: Mock()}
        with patch.object(M.base, 'process_identity', return_value=direct):
            self.assertEqual(M.direct_driver(obj), direct)
        obj.children[11] = Mock()
        with self.assertRaises(ValueError): M.direct_driver(obj)

    def test_node_context_preserves_gate_flags_and_not_credentials(self):
        env = {'GITHUB_SHA': 'abc', 'GITHUB_TOKEN': 'secret', 'PACKAGE_ACCEPTANCE_REQUESTED': 'true',
               'PACKAGE_TUN_REQUESTED': 'true', 'POLARIS_NATIVE_DNS_FORMAL': '0',
               'POLARIS_NATIVE_PAYLOAD_VALIDATION': '1', 'CORE_CANDIDATE': '',
               'POLARIS_API_SECRET': 'no', 'LD_PRELOAD': 'no', 'ALL_PROXY': 'no'}
        selected = M.node_environment(env)
        for key in ('GITHUB_SHA', 'PACKAGE_ACCEPTANCE_REQUESTED', 'PACKAGE_TUN_REQUESTED',
                    'POLARIS_NATIVE_DNS_FORMAL', 'POLARIS_NATIVE_PAYLOAD_VALIDATION', 'CORE_CANDIDATE'):
            self.assertEqual(selected[key], env[key])
        for key in ('GITHUB_TOKEN', 'POLARIS_API_SECRET', 'LD_PRELOAD', 'ALL_PROXY'):
            self.assertNotIn(key, selected)

    def test_graph_dns_requires_correct_backend_role(self):
        plan, analysis, snapshots, streams, dns = fixture_evidence()
        dns[0]['expectedBackend'] = 'B'
        with self.assertRaises(ValueError): M.join_capture(analysis, snapshots, streams, dns, plan)
        dns[0]['expectedBackend'] = 'A'
        _, joined = M.join_capture(analysis, snapshots, streams, dns, plan)
        self.assertEqual(joined[0]['backendRole'], 'A'); self.assertEqual(joined[0]['ttl'], 0)

    def test_supervised_tshark_timeout_is_forced_failure(self):
        obj = Mock(); obj.forced = []; child = Mock(); child.pid = 55
        child.communicate.side_effect = M.subprocess.TimeoutExpired('tshark', 30)
        obj.spawn.return_value = child; obj.owned = {55: identity(55, os.getuid())}
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'capture.pcap'; path.write_bytes(b'0' * 24)
            with patch.object(M.fixture.tun, 'root_path', return_value=Path('/usr/bin/tshark')), self.assertRaises(ValueError):
                M.fixture.analyze_capture(path, M.tun.topology('123456789abc'), supervisor=obj)
        self.assertEqual(obj.forced[0]['reason'], 'capture-analysis-timeout')
        obj.signal.assert_called_once_with(obj.owned[55], M.signal.SIGKILL)
        child.wait.assert_called_once_with(timeout=5)
        args, kwargs = obj.spawn.call_args
        self.assertEqual(args[0][0], '/usr/bin/tshark')
        self.assertEqual(args[2], 'capture-analysis')
        self.assertEqual(kwargs['stderr'], M.subprocess.PIPE)

    def test_supervised_tshark_failure_not_capture_pass(self):
        obj = Mock(); child = Mock(); child.pid = 55; child.returncode = 1
        child.communicate.return_value = (b'', b'private tool diagnostic')
        obj.spawn.return_value = child; obj.owned = {55: identity(55, os.getuid())}
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'capture.pcap'; path.write_bytes(b'0' * 24)
            with patch.object(M.fixture.tun, 'root_path', return_value=Path('/usr/bin/tshark')), self.assertRaises(ValueError):
                M.fixture.analyze_capture(path, M.tun.topology('123456789abc'), supervisor=obj)

    def test_mock_analysis_cannot_use_real_supervisor_branch(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'capture.pcap'; path.write_bytes(b'0' * 24)
            with self.assertRaises(ValueError):
                M.fixture.analyze_capture(path, M.tun.topology('123456789abc'), execute=Mock(), supervisor=Mock())

    def test_connectivity_probe_is_fixed_fresh_tls_without_proxy_or_tokens(self):
        source = M.CONNECTIVITY_PROBE
        self.assertIn("host = 'api.github.com'", source)
        self.assertIn('socket.getaddrinfo(host, 443', source)
        self.assertIn('ssl.create_default_context()', source)
        self.assertIn('ssl.CERT_REQUIRED', source)
        self.assertIn("connection.request('GET', '/meta'", source)
        self.assertNotIn('Authorization', source)
        self.assertNotIn('os.environ', source)
        self.assertNotIn('proxy', source.lower())

    def test_fresh_connectivity_process_timeout_is_permanent_failure(self):
        run = M.TunCarrierRun.__new__(M.TunCarrierRun); run.env = {'HOME': '/private'}
        run.supervisor = Mock(); child = Mock(); child.pid = 44
        child.communicate.side_effect = M.subprocess.TimeoutExpired('python', 15)
        run.supervisor.spawn.return_value = child; run.supervisor.owned = {44: identity(44, os.getuid())}
        run.supervisor.forced = []
        with self.assertRaises(ValueError): run.control_connectivity('after')
        self.assertEqual(run.supervisor.forced[0]['reason'], 'control-connectivity-timeout')
        run.supervisor.signal.assert_called_once_with(run.supervisor.owned[44], M.signal.SIGKILL)
        self.assertEqual(run.supervisor.spawn.call_args.args[0][:3], ['/usr/bin/python3', '-I', '-c'])

    def test_failed_control_response_not_reinterpreted_as_restored(self):
        run = M.TunCarrierRun.__new__(M.TunCarrierRun); run.env = {'HOME': '/private'}
        run.supervisor = Mock(); child = Mock(); child.pid = 44; child.returncode = 0
        child.communicate.return_value = (b'{"host":"api.github.com","dnsResolved":true,"httpsStatus":403,"tlsVerified":true}', None)
        run.supervisor.spawn.return_value = child; run.supervisor.owned = {44: identity(44, os.getuid())}
        with self.assertRaises(ValueError): run.control_connectivity('after')

    def test_root_gui_binding_must_match_actual_app_inode_and_hash(self):
        app = {**identity(10, os.getuid()), 'sha256': H}
        proof = {'identity': copy.deepcopy(app), 'executable': {'dev': app['exeDev'], 'inode': app['exeInode'], 'sha256': H}}
        self.assertEqual(M.verify_gui_binding(app, proof), proof)
        for target, key, bad in (('identity', 'pid', 11), ('identity', 'startTime', '900'),
                                 ('identity', 'uid', 0), ('identity', 'exeInode', 900),
                                 ('executable', 'dev', 900), ('executable', 'inode', 900),
                                 ('executable', 'sha256', 'b' * 64)):
            invalid = copy.deepcopy(proof); invalid[target][key] = bad
            with self.assertRaises(ValueError): M.verify_gui_binding(app, invalid)
        with self.assertRaises(ValueError): M.verify_gui_binding(app, {'identity': app})

    def test_app_inode_is_read_from_owned_open_executable(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / 'app'; raw = b'\x7fELFactual-test-byte-stream'; path.write_bytes(raw)
            info = path.stat(); fd = os.open(path, os.O_RDONLY)
            app = {**identity(10, os.getuid()), 'sha256': hashlib.sha256(raw).hexdigest()}
            with patch.object(M.base, 'process_identity', return_value=app), \
                 patch.object(M.os, 'open', return_value=fd), patch.object(M.Path, 'stat', return_value=info):
                result = M.app_execution_identity(app)
            self.assertEqual(result['exeDev'], info.st_dev)
            self.assertEqual(result['exeInode'], info.st_ino)
            self.assertEqual(result['sha256'], app['sha256'])

    def test_only_two_fixed_root_rpc_methods(self):
        run = M.TunCarrierRun.__new__(M.TunCarrierRun); run.controller = Mock()
        run.controller.call.return_value = {'schema': 'polaris-package-rpc-probe-v1', 'ok': True,
                                           'action': 'tun-capabilities', 'result': {'active': {}}}
        self.assertEqual(run.rpc('capabilities'), {'active': {}})
        run.controller.call.assert_called_once_with('rpc', method='capabilities')
        for action in ('suite', 'apply', 'retry', 'ack', 'commit'):
            with self.assertRaises(ValueError): run.rpc(action)
        with self.assertRaises(ValueError): run.rpc('status', {'transactionId': 'x'})

    def test_rpc_wrong_envelope_never_success(self):
        run = M.TunCarrierRun.__new__(M.TunCarrierRun); run.controller = Mock()
        for response in ({'ok': False}, {'schema': 'polaris-package-rpc-probe-v1', 'ok': True, 'action': 'suite'}):
            run.controller.call.return_value = response
            with self.assertRaises(ValueError): run.rpc('capabilities')

    def test_core_off_required_before_tray_shortcut(self):
        run = M.TunCarrierRun.__new__(M.TunCarrierRun); run.driver = Mock()
        run.stopped = False
        with self.assertRaises(ValueError): run.quit_product()
        run.driver.quit_shortcut.assert_not_called()

    def test_unknown_cleanup_or_forced_never_passes(self):
        for cleanup in ({'state': 'normal', 'forced': False}, {'state': 'failed', 'forced': True},
                        {'state': 'normal', 'forced': False, 'unknown': ['lost-child']}):
            result = M.tun.report_result({}, cleanup, 'same-final-carrier')
            self.assertNotEqual(result['result'], 'success')
            self.assertFalse(result['publicationEligible'])

    def test_case_full_identity_required_not_pid_only(self):
        evidence = {'beforeCore': M.birth(identity()), 'afterCore': M.birth(identity()),
                    'newConnection': {'exit': 'B', 'bytes': 2, 'nonceSha256': H}}
        with self.assertRaises(ValueError): M.tun.validate_case('daily_switch', evidence)
        evidence.update(beforeCore=identity(), afterCore=identity())
        M.tun.validate_case('daily_switch', evidence)

    def test_each_tun_case_is_declared(self):
        source = (HERE / 'linux_tun.py').read_text()
        for name in M.tun.CASE_IDS:
            self.assertIn("'" + name + "'", source)
        self.assertIn('self.supervisor.close(self.normal_quit and root_finish is not None)', source)
        self.assertIn("root_finish['watchdogCancelled']", source)
        self.assertNotIn("'watchdogCancelled': True", source)


if __name__ == '__main__':
    unittest.main()
