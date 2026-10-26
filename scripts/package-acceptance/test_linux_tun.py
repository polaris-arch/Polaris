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
import subprocess
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
for name, directory in (('linux_tun.py', HERE), ('linux.py', HERE), ('helper_observation.py', HERE), ('tun.py', SUPPORT),
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


def helper_row(success=True, code='missing', installed=None, ready=None):
    status={key:None for key in M.helper_observation.STATUS_BOOLS}
    status.update(supported=True,installed=success if installed is None else installed,ready=success if ready is None else ready)
    return {'sequence':1,'kind':'helperObservation','observedElapsedMs':735,
        'projection':{'schema':M.helper_observation.SCHEMA,'kind':'helperAction','association':'shape-only',
            'envelopeSuccess':True,'actionSuccess':success,'errorCode':code,'statusShape':'valid','status':status,
            'selectedCoreState':'missing','protocolVersion':None,'expectedProtocolVersion':None,'buildIdentityMatches':None,'executionDiagnosticState':'absent','executionDiagnostic':None}}


def transaction_binding(run,kind='install'):
    run.transaction_launch={'runId':'a'*12,'execution':{'sourceSha':'b'*40,'githubRunId':'1','githubRunAttempt':'1'}}
    value={'schema':'polaris-transaction-binding-v1','runToken':'a'*12,'sourceSha':'b'*40,'githubRunId':'1','attempt':'1',
           'kind':kind,'subject':{k:run.app[k] for k in ('pid','startTime','uid')},'windowNonce':'c'*32}
    run.transaction_binding=value
    return value


def begin_snapshot(run,kind='install'):
    return {'observing':kind,'transactionBinding':transaction_binding(run,kind)}


def scan_snapshot(run,kind='install'):
    import root_controller
    runtime=root_controller.auth_runtime_observation(run.launch_options['authenticationMode']);runtime['kind']=kind
    return {'kind':kind,'subject':{key:run.app[key] for key in ('pid','startTime','uid')},
        'authenticationMode':run.launch_options['authenticationMode'],'pkexecBirths':[], 'rootShellBirths':[],
        'owned':[],'authorizationObserved':False,'credentialPromptClaimed':False,'authenticationRuntime':runtime,
        'transactionBinding':transaction_binding(run,kind),'auxiliaryIdentityCoverage':'complete','auxiliaryTerminals':[]}


def end_snapshot(run,kind='install'):
    value=scan_snapshot(run,kind);value.pop('authenticationRuntime')
    value['closed']=True
    return value


def bind_supervisor_proof(obj,owned):
    run=SimpleNamespace(app=obj.window['subject'])
    binding=transaction_binding(run,obj.window['kind']);obj.bind_window(binding,run.transaction_launch)
    key=[owned[0]['pid'],owned[0]['startTime']]
    return {'closed':True,'kind':obj.window['kind'],'subject':dict(obj.window['subject']),
            'transactionBinding':binding,'owned':owned,'pkexecBirths':[key],'rootShellBirths':[key],
            'authorizationObserved':True,'auxiliaryIdentityCoverage':'complete','auxiliaryTerminals':[]}


def supervisor():
    obj = M.TunSupervisor.__new__(M.TunSupervisor)
    obj.lock = threading.RLock(); obj.window = None; obj.quarantine = {}; obj.confirmed = set(); obj.auxiliary_confirmed={}
    obj.controller_birth = None; obj.controller_descendants = {}; obj.gui_birth = None
    obj.owned = {}; obj.children = {}; obj.roles = {}; obj.unknown = []; obj.forced = []
    obj.diagnostics = []; obj.diagnostics_truncated = False
    obj.diagnostic_executable_budget = [M.base.DIAGNOSTIC_EXE_LIMIT]
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
    def test_historical_admission_is_rejected_before_carrier_or_product(self):
        self.assertEqual(M.tun.KERNEL_COMMIT, '0cfa24573ff34c26608151068334329268cfdf14')
        for commit in ('c6c0d8b3b83ab4d6e7b953dd2dbdb7efe6f8021a',
                       '814da362838e96e54f904836a92c2b0d8c3ae29b'):
            with self.subTest(commit=commit), tempfile.TemporaryDirectory() as tmp:
                path = Path(tmp) / 'historical-admission.json'
                original = json.dumps({'schema': 'polaris-linux-tun-admission-v1',
                    'kernelCommit': commit, 'sourceScope': 'same-final-carrier',
                    'publicationEligible': False}).encode()
                path.write_bytes(original)
                with patch.object(M.base, 'carrier_path') as carrier, patch.object(M, 'TunCarrierRun') as run:
                    with self.assertRaisesRegex(ValueError, '^real final-carrier TUN admission required$'):
                        M.run_all(SimpleNamespace(admission=path))
                    carrier.assert_not_called(); run.assert_not_called()
                self.assertEqual(path.read_bytes(), original)

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
        obj.end_window(bind_supervisor_proof(obj,[root]))
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
        obj.end_window(bind_supervisor_proof(obj,[elevated]))

    def test_missed_root_ancestry_fails_closed(self):
        obj = supervisor(); obj.begin_window('install', identity())
        obj.window['births'].add((11, '1234'))
        with self.assertRaises(ValueError): obj.end_window(bind_supervisor_proof(obj,[identity(12)]))
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
            observed = {'pid':10,'startTime':'123','ppid':2,'state':'S','uid':os.getuid()}
            app = {**observed, 'exe':str(path), 'sha256': hashlib.sha256(raw).hexdigest()}
            with patch.object(M.base, 'process_identity', return_value=observed), \
                 patch.object(M.os, 'open', return_value=fd), patch.object(M.Path, 'stat', return_value=info):
                result = M.app_execution_identity(app)
            self.assertEqual(result['exeDev'], info.st_dev)
            self.assertEqual(result['exeInode'], info.st_ino)
            self.assertEqual(result['sha256'], app['sha256'])

    def test_only_two_fixed_root_rpc_methods(self):
        run = M.TunCarrierRun.__new__(M.TunCarrierRun); run.controller = Mock()
        run.revalidate_api_endpoint = Mock()
        run.controller.call.return_value = {'schema': 'polaris-package-rpc-probe-v1', 'ok': True,
                                           'action': 'tun-capabilities', 'result': {'active': {}}}
        self.assertEqual(run.rpc('capabilities'), {'active': {}})
        run.controller.call.assert_called_once_with('rpc', method='capabilities')
        for action in ('suite', 'apply', 'retry', 'ack', 'commit'):
            with self.assertRaises(ValueError): run.rpc(action)
        with self.assertRaises(ValueError): run.rpc('status', {'transactionId': 'x'})

    def test_rpc_wrong_envelope_never_success(self):
        run = M.TunCarrierRun.__new__(M.TunCarrierRun); run.controller = Mock()
        run.revalidate_api_endpoint = Mock()
        for response in ({'ok': False}, {'schema': 'polaris-package-rpc-probe-v1', 'ok': True, 'action': 'suite'}):
            run.controller.call.return_value = response
            with self.assertRaises(ValueError): run.rpc('capabilities')

    def test_core_off_required_before_tray_shortcut(self):
        run = M.TunCarrierRun.__new__(M.TunCarrierRun); run.result = {}; run.driver = Mock()
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


class PrivateProbeContracts(unittest.TestCase):
    def test_source_size_bounds_reject_before_read_or_target_creation(self):
        for size in (0, 128 * 1024 * 1024 + 1):
            with self.subTest(size=size), tempfile.TemporaryDirectory() as tmp:
                home = Path(tmp); source = home / 'source'
                with source.open('wb') as stream: stream.truncate(size)
                with patch.object(M.os, 'pread') as reread:
                    with self.assertRaisesRegex(ValueError, 'private probe source metadata invalid'):
                        M.private_root_probe(source, home, hashlib.sha256(b'probe').hexdigest())
                reread.assert_not_called()
                self.assertFalse((home / 'root-rpc-probe').exists())

    def test_same_observed_timestamps_do_not_hide_source_content_changes(self):
        # Model timestamp collisions, not the real CI filesystem's unrecorded
        # timestamps. Every mutation writes real bytes; reads are never cached
        # or replaced with fixture bytes. Only this source's timestamp fields
        # are held constant, leaving inode/owner/mode/size/link checks active.
        for fault in (None, 'after-read', 'first-reread', 'during-copy', 'thread-copy', 'final-reread'):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as tmp:
                home = Path(tmp); source = home / 'source'; source.write_bytes(b'probe')
                initial = source.lstat()
                real_stat, real_lstat = os.fstat, Path.lstat
                real_read, real_sync, real_open = os.pread, os.fsync, os.open
                counts = {}; opened = []; reads = []; changed = []; source_fd = []
                def same_timestamps(value):
                    if (value.st_dev, value.st_ino) != (initial.st_dev, initial.st_ino): return value
                    fields = {key: getattr(value, key) for key in dir(value) if key.startswith('st_')}
                    fields.update(st_mtime_ns=initial.st_mtime_ns, st_ctime_ns=initial.st_ctime_ns)
                    return SimpleNamespace(**fields)
                def change():
                    if fault == 'thread-copy':
                        errors = []
                        def writer():
                            try: source.write_bytes(b'other')
                            except Exception as error: errors.append(error)
                        worker = threading.Thread(target=writer)
                        worker.start(); worker.join(timeout=5)
                        self.assertFalse(worker.is_alive()); self.assertEqual(errors, [])
                    else: source.write_bytes(b'other')
                    changed.append(True)
                def observed_stat(fd):
                    value = real_stat(fd)
                    if (value.st_dev, value.st_ino) == (initial.st_dev, initial.st_ino):
                        counts[fd] = counts.get(fd, 0) + 1
                        if not source_fd: source_fd.append(fd)
                        if fault == 'after-read' and counts[fd] == 2:
                            change(); value = real_stat(fd)
                    return same_timestamps(value)
                def observed_lstat(path, *args, **kwargs):
                    return same_timestamps(real_lstat(path, *args, **kwargs))
                def read_current(fd, size, offset):
                    self.assertEqual(fd, source_fd[0])
                    self.assertEqual((size, offset), (initial.st_size + 1, 0))
                    data = real_read(fd, size, offset); reads.append(data)
                    if (fault == 'first-reread' and len(reads) == 1) or (fault == 'final-reread' and len(reads) == 3): change()
                    return data
                def sync_target(fd):
                    self.assertNotEqual(fd, source_fd[0])
                    self.assertEqual(real_stat(source_fd[0]).st_ino, initial.st_ino)
                    real_sync(fd)
                    if fault in ('during-copy', 'thread-copy'): change()
                def opened_fd(*args, **kwargs):
                    fd = real_open(*args, **kwargs); opened.append(fd); return fd
                with patch.object(M.os, 'fstat', side_effect=observed_stat), \
                     patch.object(M.Path, 'lstat', observed_lstat), \
                     patch.object(M.os, 'pread', side_effect=read_current), \
                     patch.object(M.os, 'fsync', side_effect=sync_target), \
                     patch.object(M.os, 'open', side_effect=opened_fd):
                    if fault is None:
                        copied = M.private_root_probe(source, home, hashlib.sha256(b'probe').hexdigest())
                        self.assertEqual(copied.read_bytes(), b'probe')
                    else:
                        with self.assertRaisesRegex(ValueError, 'private probe source changed'):
                            M.private_root_probe(source, home, hashlib.sha256(b'probe').hexdigest())
                self.assertEqual(changed, [] if fault is None else [True])
                self.assertEqual(source.read_bytes(), b'probe' if fault is None else b'other')
                self.assertEqual(len(reads), {None: 4, 'after-read': 1, 'first-reread': 2, 'during-copy': 3, 'thread-copy': 3, 'final-reread': 4}[fault])
                target = home / 'root-rpc-probe'
                self.assertEqual(target.exists(), fault in (None, 'during-copy', 'thread-copy', 'final-reread'))
                if target.exists(): self.assertEqual(target.read_bytes(), b'probe')
                for fd in opened:
                    with self.assertRaises(OSError): real_stat(fd)
                self.assertIs(os.fstat, real_stat); self.assertIs(Path.lstat, real_lstat)
                self.assertIs(os.pread, real_read); self.assertIs(os.fsync, real_sync)
                self.assertIs(os.open, real_open)

    def test_bounded_source_reread_faults_fail_closed_and_close_descriptors(self):
        for phase in ('before-copy', 'after-copy'):
            for fault in ('short-read', 'read-error', 'truncate', 'grow', 'replace', 'symlink', 'mode', 'unlink'):
                with self.subTest(phase=phase, fault=fault), tempfile.TemporaryDirectory() as tmp:
                    home = Path(tmp); source = home / 'source'; source.write_bytes(b'probe')
                    replacement = home / 'replacement'; replacement.write_bytes(b'probe')
                    real_read, real_open, real_stat = os.pread, os.open, os.fstat
                    opened = []; calls = []; changed = []
                    def read_current(fd, size, offset):
                        self.assertEqual((size, offset), (6, 0)); calls.append(fd)
                        if len(calls) == (1 if phase == 'before-copy' else 3):
                            changed.append(True)
                            if fault == 'short-read': return real_read(fd, size, offset)[:-1]
                            if fault == 'read-error': raise OSError('controlled pread failure')
                            if fault == 'truncate': source.write_bytes(b'pr')
                            if fault == 'grow': source.write_bytes(b'probe-extra')
                            if fault == 'replace': replacement.replace(source)
                            if fault == 'symlink': source.unlink(); source.symlink_to(replacement)
                            if fault == 'mode': source.chmod(0o777)
                            if fault == 'unlink': source.unlink()
                        return real_read(fd, size, offset)
                    def opened_fd(*args, **kwargs):
                        fd = real_open(*args, **kwargs); opened.append(fd); return fd
                    with patch.object(M.os, 'pread', side_effect=read_current), patch.object(M.os, 'open', side_effect=opened_fd):
                        with self.assertRaises(OSError if fault == 'read-error' else ValueError):
                            M.private_root_probe(source, home, hashlib.sha256(b'probe').hexdigest())
                    self.assertEqual(changed, [True])
                    target = home / 'root-rpc-probe'
                    self.assertEqual(target.exists(), phase == 'after-copy')
                    if target.exists(): self.assertEqual(target.read_bytes(), b'probe')
                    for fd in opened:
                        with self.assertRaises(OSError): real_stat(fd)
                    self.assertIs(os.pread, real_read); self.assertIs(os.open, real_open)

    def test_hardlinked_cargo_source_gets_independent_single_link_copy(self):
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp); source = home / 'hashed'; source.write_bytes(b'cargo-probe')
            alias = home / 'example'; os.link(source, alias)
            copied = M.private_root_probe(alias, home, hashlib.sha256(b'cargo-probe').hexdigest())
            self.assertEqual(alias.stat().st_nlink, 2)
            self.assertEqual(copied.stat().st_nlink, 1)
            self.assertNotEqual(copied.stat().st_ino, alias.stat().st_ino)
            self.assertEqual(copied.read_bytes(), alias.read_bytes())
            self.assertEqual(copied.stat().st_mode & 0o777, 0o500)
            with self.assertRaises(FileExistsError): M.private_root_probe(alias, home, M.base.sha(alias))

    def test_source_symlink_hash_mismatch_and_unprivate_directory_rejected(self):
        for fault in ('symlink', 'hash', 'directory'):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as tmp:
                home = Path(tmp); source = home / 'source'; source.write_bytes(b'probe')
                if fault == 'symlink':
                    alias = home / 'alias'; alias.symlink_to(source); source = alias
                if fault == 'directory': home.chmod(0o755)
                digest = '0' * 64 if fault == 'hash' else M.base.sha(source)
                with self.assertRaises(ValueError): M.private_root_probe(source, home, digest)
                self.assertFalse((home / 'root-rpc-probe').exists())

    def test_partial_write_failure_retained_without_source_mutation(self):
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp); source = home / 'source'; source.write_bytes(b'probe')
            with patch.object(M.os, 'fsync', side_effect=OSError('PRIVATE')):
                with self.assertRaises(OSError): M.private_root_probe(source, home, M.base.sha(source))
            self.assertEqual(source.read_bytes(), b'probe')
            self.assertTrue((home / 'root-rpc-probe').exists())

    def test_foreign_owner_and_source_changed_during_copy_rejected(self):
        for fault in ('owner', 'late-change'):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as tmp:
                home = Path(tmp); source = home / 'source'; source.write_bytes(b'probe')
                real_sync = os.fsync
                def change_source(fd):
                    real_sync(fd); source.write_bytes(b'other')
                context = patch.object(M.os, 'getuid', return_value=os.getuid() + 1) if fault == 'owner' else patch.object(M.os, 'fsync', side_effect=change_source)
                with context:
                    with self.assertRaises(ValueError): M.private_root_probe(source, home, hashlib.sha256(b'probe').hexdigest())

    def test_source_replacement_between_lstat_and_open_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            home = Path(tmp); source = home / 'source'; source.write_bytes(b'probe')
            replacement = home / 'replacement'; replacement.write_bytes(b'probe')
            real_open = os.open
            def racing_open(path, *args, **kwargs):
                if Path(path) == source: replacement.replace(source)
                return real_open(path, *args, **kwargs)
            with patch.object(M.os, 'open', side_effect=racing_open):
                with self.assertRaises(ValueError): M.private_root_probe(source, home, hashlib.sha256(b'probe').hexdigest())
            self.assertFalse((home / 'root-rpc-probe').exists())

    def test_source_mutation_after_read_and_target_replacement_rejected(self):
        for fault in ('source', 'target'):
            with self.subTest(fault=fault), tempfile.TemporaryDirectory() as tmp:
                home = Path(tmp); source = home / 'source'; source.write_bytes(b'probe')
                original = os.fstat; counts = {}
                def racing_stat(fd):
                    counts[fd] = counts.get(fd, 0) + 1
                    value = original(fd)
                    if fault == 'source' and value.st_ino == source.stat().st_ino and counts[fd] == 2:
                        source.write_bytes(b'other')
                        return original(fd)
                    return value
                real_sync = os.fsync
                def racing_sync(fd):
                    real_sync(fd)
                    if fault == 'target':
                        target = home / 'root-rpc-probe'; target.unlink(); target.write_bytes(b'other')
                with patch.object(M.os, 'fstat', side_effect=racing_stat), patch.object(M.os, 'fsync', side_effect=racing_sync):
                    with self.assertRaises(ValueError): M.private_root_probe(source, home, hashlib.sha256(b'probe').hexdigest())
                if fault == 'target': self.assertEqual((home / 'root-rpc-probe').read_bytes(), b'other')


class ControllerLifecycleContracts(unittest.TestCase):
    def test_abort_full_validated_receipt_retained_but_never_success(self):
        import root_controller
        receipt = {'schema': 'polaris-root-abort-v1', 'state': 'failed', 'forced': True, 'prepared': False,
                   'actualReadOnlyAuth': root_controller.auth_observation(), 'unknown': []}
        for fault in (None, 'extra', 'normal', 'unknown', 'missing', 'nested'):
            with self.subTest(fault=fault):
                value = copy.deepcopy(receipt)
                if fault == 'extra': value['secret'] = 'PRIVATE'
                if fault == 'normal': value['state'] = 'normal'
                if fault == 'unknown': value['unknown'] = ['PRIVATE']
                if fault == 'missing': del value['prepared']
                if fault == 'nested': value['actualReadOnlyAuth']['secret'] = 'PRIVATE'
                client = M.Controller(); client.child = Mock(); client.child.wait.return_value = 1
                client.lifecycle.update(ready=True, spawned=True)
                with patch.object(client, 'call', return_value=value): result = client.recover()
                self.assertEqual(result['state'], 'failed'); self.assertTrue(result['forced'])
                self.assertNotIn('privilegedProcessesReaped', result)
                if fault:
                    self.assertIn('root-abort-unconfirmed', result['unknown'])
                    self.assertNotIn('abortReceipt', client.lifecycle)
                else:
                    self.assertEqual(client.lifecycle['abortReceipt'], receipt)
                    self.assertTrue(client.lifecycle['abortAcknowledged'])
                    self.assertIn('exception-required-recovery', result['unknown'])
                self.assertNotIn('PRIVATE', json.dumps(client.lifecycle))

    def test_prepared_and_partial_abort_outcomes_are_not_discarded(self):
        for prepared in (True, None):
            with self.subTest(prepared=prepared):
                receipt = {'schema': 'polaris-root-abort-v1', 'state': 'failed', 'forced': True,
                    'prepared': prepared, 'actualReadOnlyAuth': None,
                    'unknown': [] if prepared else ['root-preparation-incomplete']}
                if prepared: receipt['productCleanup'] = {'state': 'forced', 'forced': True,
                    'unknown': ['network-restoration-unproved'], 'publicationEligible': False}
                client = M.Controller(); client.child = Mock(); client.child.wait.return_value = 1
                client.lifecycle.update(ready=True, spawned=True)
                with patch.object(client, 'call', return_value=receipt): result = client.recover()
                self.assertEqual(client.lifecycle['abortReceipt'], receipt)
                self.assertEqual(result['state'], 'failed'); self.assertTrue(result['forced'])
                self.assertNotIn('privilegedProcessesReaped', result)

    def test_auth_failure_observation_is_strictly_validated_without_raw_error(self):
        import root_controller
        for bad in (False, True):
            client = M.Controller(); observation = root_controller.auth_observation()
            if bad: observation['raw'] = 'PRIVATE'
            response = {'seq': 1, 'ok': False, 'error': {'stage': 'authentication-readiness', 'code': 'CONTROLLER_FAILED',
                'actualReadOnlyAuth': observation, 'message': 'PRIVATE'}}
            with patch.object(client, '_write'), patch.object(client, '_read', return_value=response):
                with self.assertRaises(ValueError): client.call('prepare')
            exported = client.lifecycle['rootOperationFailure']
            self.assertEqual('actualReadOnlyAuth' in exported, not bad)
            self.assertNotIn('PRIVATE', json.dumps(client.lifecycle))

    def test_allocation_never_spawns(self):
        with patch.object(M.subprocess, 'Popen') as popen:
            client = M.Controller()
        popen.assert_not_called()
        self.assertIsNone(client.child)
        self.assertFalse(client.lifecycle['spawned'])

    def test_pre_ready_failure_matrix_retains_handle_and_never_signals_root(self):
        for fault in ('envelope', 'spawn', 'birth', 'write-envelope', 'malformed', 'eof', 'timeout', 'wrong-ready', 'root-failure'):
            with self.subTest(fault=fault):
                client = M.Controller(); sup = supervisor(); child = Mock(); child.pid = 901
                child.wait.return_value = 70
                launch = {'runId': '123456789abc'}
                sources = {'source': b'x' * (2 * 1024 * 1024 if fault == 'envelope' else 1)}
                ready = {'schema': 'polaris-tun-controller-v1', 'ready': True, 'runId': launch['runId']}
                if fault == 'wrong-ready': ready['runId'] = '000000000000'
                if fault == 'root-failure': ready = {'schema': 'polaris-tun-controller-v1', 'ready': False,
                    'runId': launch['runId'], 'startupFailure': {'stage': 'probe', 'code': 'BOOTSTRAP_FAILED',
                        'stagingCleanup': 'removed', 'secret': 'NEVER-EXPORT'}}
                read_error = ValueError('NEVER-EXPORT') if fault in ('malformed', 'eof', 'timeout') else None
                with patch.object(M.subprocess, 'Popen', return_value=child,
                                  side_effect=OSError('NEVER-EXPORT') if fault == 'spawn' else None) as popen, \
                     patch.object(M.base, 'process_identity', return_value=None if fault == 'birth' else identity(901)), \
                     patch.object(client, '_write', side_effect=BrokenPipeError('NEVER-EXPORT') if fault == 'write-envelope' else None), \
                     patch.object(client, '_read', return_value=ready, side_effect=read_error), \
                     patch.object(M.os, 'kill') as kill:
                    with self.assertRaises(Exception): client.start(launch, sources, sup, {})
                    result = client.recover()
                kill.assert_not_called()
                self.assertEqual(result['state'], 'failed')
                self.assertFalse(client.lifecycle['ready'])
                self.assertNotIn('NEVER-EXPORT', json.dumps(client.lifecycle))
                if fault in ('envelope', 'spawn'):
                    child.stdin.close.assert_not_called(); child.wait.assert_not_called()
                else:
                    self.assertIs(client.child, child)
                    child.stdin.close.assert_called_once(); child.wait.assert_called_once_with(timeout=20)
                    self.assertTrue(client.closed)
                if fault == 'root-failure':
                    self.assertEqual(client.lifecycle['rootStartupFailure']['stage'], 'probe')

    def test_ready_abort_failure_and_incomplete_exit_are_unknown(self):
        for fault in ('abort', 'input-close', 'wait'):
            with self.subTest(fault=fault):
                client = M.Controller(); client.child = Mock()
                client.child.wait.return_value = 1
                client.lifecycle.update(spawned=True, ready=True)
                if fault == 'input-close': client.child.stdin.close.side_effect = OSError('PRIVATE')
                if fault == 'wait': client.child.wait.side_effect = M.subprocess.TimeoutExpired('fixed-child', 20)
                with patch.object(client, 'call', return_value={'unknown': []},
                                  side_effect=ValueError('PRIVATE') if fault == 'abort' else None) as call, \
                     patch.object(M.os, 'kill') as kill:
                    result = client.recover()
                call.assert_called_once_with('abort'); kill.assert_not_called()
                self.assertEqual(result['state'], 'failed')
                self.assertGreater(len(result['unknown']), 1)
                self.assertEqual(client.closed, fault != 'wait')
                self.assertNotIn('PRIVATE', json.dumps(client.lifecycle))

    def test_real_pipe_read_eof_malformed_timeout_and_buffered_frames(self):
        # Actual unprivileged anonymous pipes only: no child/product/root process.
        for mode in ('frames', 'eof', 'malformed', 'timeout'):
            with self.subTest(mode=mode):
                read_fd, write_fd = os.pipe()
                client = M.Controller()
                client.child = SimpleNamespace(stdout=os.fdopen(read_fd, 'rb'))
                try:
                    if mode == 'frames': os.write(write_fd, b'{"one":1}\n{"two":2}\n')
                    elif mode == 'malformed': os.write(write_fd, b'PRIVATE-invalid-json\n')
                    elif mode == 'eof': os.close(write_fd); write_fd = -1
                    if mode == 'frames':
                        self.assertEqual(client._read(0.05), {'one': 1})
                        self.assertEqual(client._read(0.05), {'two': 2})
                    else:
                        with self.assertRaises(ValueError): client._read(0.01)
                finally:
                    if write_fd >= 0: os.close(write_fd)
                    client.child.stdout.close()

    def test_real_pipe_write_is_bounded_json_line(self):
        read_fd, write_fd = os.pipe(); client = M.Controller()
        client.child = SimpleNamespace(stdin=os.fdopen(write_fd, 'wb'))
        try:
            client._write({'seq': 1, 'action': 'abort'})
            self.assertEqual(json.loads(os.read(read_fd, 4096)), {'seq': 1, 'action': 'abort'})
        finally:
            client.child.stdin.close(); os.close(read_fd)

    def test_bind_failure_frame_is_allowlisted_and_unknown_fields_are_not_exported(self):
        client = M.Controller(); client.lifecycle['ready'] = True
        response = {'seq': 1, 'ok': False, 'error': {'stage': 'bind-gui', 'code': 'CONTROLLER_FAILED',
                                                   'message': 'PRIVATE', 'secret': 'PRIVATE'}}
        with patch.object(client, '_write'), patch.object(client, '_read', return_value=response):
            with self.assertRaises(ValueError): client.call('bind_gui', pid=10)
        self.assertEqual(client.lifecycle['rootOperationFailure'], {'stage': 'bind-gui', 'code': 'CONTROLLER_FAILED'})
        self.assertNotIn('PRIVATE', json.dumps(client.lifecycle))
        client = M.Controller(); response['error']['stage'] = 'PRIVATE'
        with patch.object(client, '_write'), patch.object(client, '_read', return_value=response):
            with self.assertRaises(ValueError): client.call('bind_gui', pid=10)
        self.assertNotIn('rootOperationFailure', client.lifecycle)

    def test_finish_wait_failure_keeps_handle_open_for_recovery(self):
        client = M.Controller(); client.child = Mock(); client.lifecycle.update(spawned=True, ready=True)
        client.child.wait.side_effect = [M.subprocess.TimeoutExpired('fixed-child', 15), 0]
        with patch.object(client, 'call', return_value={'unknown': []}):
            with self.assertRaises(M.subprocess.TimeoutExpired): client.finish()
            self.assertFalse(client.closed)
            result = client.recover()
        self.assertTrue(client.closed)
        self.assertEqual(result['state'], 'failed')
        self.assertNotIn('privilegedProcessesReaped', result)

    def test_start_controller_assigns_before_start_can_fail(self):
        import root_controller
        with tempfile.TemporaryDirectory() as tmp:
            run = M.TunCarrierRun.__new__(M.TunCarrierRun)
            run.home = Path(tmp); directory = run.home / 'config/com.polaris.app/polaris'; directory.mkdir(parents=True)
            run.env = {'XDG_CONFIG_HOME': str(run.home / 'config')}; run.supervisor = supervisor()
            run.result = {}; run.run_id = '123456789abc'; run.rpc_client = run.home / 'probe'; run.rpc_client.write_bytes(b'probe')
            run.members = {'app': {'sha256': H}, 'core': {'sha256': H}}
            run.admission = {'kernelCommit': M.tun.KERNEL_COMMIT}
            run.bound = {'helperBinding': {'helper': {'sha256': H}}}; run.item = {'sha256': H}; run.launch_options = {'execution':{'sourceSha':'b'*40,'githubRunId':'1','githubRunAttempt':'1'}}
            run.controller = None
            client = Mock(); client.lifecycle = {'stage': 'allocated'}
            client.start.side_effect = ValueError('PRIVATE')
            source = Mock(); source.read_bytes.return_value = b'frozen-test'
            with patch.object(M, 'direct_driver', return_value=identity()), \
                 patch.object(M.base, 'regular', return_value=source), patch.object(M.base, 'sha', return_value=H), \
                 patch.object(root_controller, 'validate_launch') as validate_launch, patch.object(M, 'private_root_probe', return_value=run.rpc_client), \
                 patch.object(M, 'Controller', return_value=client):
                with self.assertRaises(ValueError): run.start_controller()
            self.assertEqual(validate_launch.call_args.args[0]['kernelCommit'], run.admission['kernelCommit'])
            self.assertIs(run.controller, client)
            self.assertIs(run.result['controllerLifecycle'], client.lifecycle)
            self.assertEqual(run.result['rootPreparationProgress']['stage'], 'controller-start')

    def test_tun_rejections_keep_original_unknown_and_add_bounded_diagnostics(self):
        obj = supervisor(); gui = identity(10, os.getuid()); foreign = identity(11, os.getuid() + 1, 10)
        obj.owned[10] = gui; obj.roles[10] = 'product'
        with patch.object(M.Path, 'iterdir', return_value=[Path('/proc/10'), Path('/proc/11')]), \
             patch.object(M.base, 'process_identity', side_effect=lambda pid: {10: gui, 11: foreign}.get(pid)), \
             patch.object(obj, 'record_diagnostic') as diagnostic:
            obj.scan()
        self.assertIn('foreign-descendant-outside-exact-GUI-transaction', obj.unknown)
        self.assertNotIn(11, obj.owned)
        self.assertEqual([call.args[0] for call in diagnostic.call_args_list],
                         ['foreign-descendant-outside-exact-gui-transaction', 'tun-privileged-birth-quarantined'])
        self.assertTrue(all(call.kwargs['stage'] == 'tun-scan' for call in diagnostic.call_args_list))


    def tun_connect_fixture(self):
        run = M.TunCarrierRun.__new__(M.TunCarrierRun)
        run.result, run.home, run.runtime_path = {}, Path('/tmp'), Path('/tmp/singbox-runtime.json')
        core = identity(); core['sha256'] = H
        run.members = {'core': {'sha256': H}}
        run.app = {'pid': 7, 'startTime': '10', 'sha256': H, 'exeDev': 1, 'exeInode': 7}
        run.status = Mock(return_value={'running': False}); run.driver = Mock(); run.controller = Mock()
        run.controller.call.return_value = {'identity': core, 'executable': {'sha256': H}, 'runtime': {'sha256': H}}
        endpoint = {'host': '127.0.0.1', 'port': 45001, 'pid': core['pid'], 'startTime': core['startTime']}
        graph = {'inbounds': [{'type': 'tun', 'interface_name': 'polaris-tun0'}]}
        run.observed_api_endpoint = Mock(return_value=(endpoint, {'sha256': H}, graph))
        run.rpc = Mock()
        status = {'running': True, 'startedViaHelper': True, 'pid': core['pid'], 'mainGeneration':1, 'clashApiPort': 45001}
        run.status=Mock(side_effect=lambda: {'running':False} if run.result.get('connectObservation',{}).get('phase')=='stopped-check' else status)
        return run, status, endpoint, graph

    def test_tun_actual_endpoint_connect_reuses_root_owned_probe_and_rebinds(self):
        run, status, endpoint, graph = self.tun_connect_fixture()
        with patch.object(run,'wait_ready_status',return_value=status), patch.object(M.base, 'regular', side_effect=lambda p,*args: p):
            run.connect()
            self.assertEqual(run.api_endpoint, endpoint)
            self.assertTrue(run.result['apiEndpoint']['ownedListenerProbe'])
            run.rpc.assert_called_once_with('capabilities')
            self.assertIn('tun_register', [c.args[0] for c in run.controller.call.call_args_list])
            second = {**endpoint, 'port':45002}
            run.observed_api_endpoint.return_value = (second, {'sha256':H}, graph)
            run.connect()
            self.assertEqual(run.api_endpoint['port'],45002)

    def test_apply_restart_retires_old_registration_before_binding_new_core_and_tun(self):
        run,status,endpoint,graph=self.tun_connect_fixture()
        previous=identity(9);calls=[]
        attested=run.controller.call.return_value
        def call(action):
            calls.append(action)
            return {'stopped':True,'identity':previous} if action=='core_stopped' else attested
        run.controller.call.side_effect=call
        run.retire_applied_core(previous)
        run.connect_phase('apply-ready')
        with patch.object(run,'wait_ready_status',return_value=status),patch.object(M.base,'regular',side_effect=lambda p,*a:p):
            run.observe_ready_core(timeout=10)
        self.assertEqual(calls,['core_stopped','register_core','core_attest','core_attest','tun_register'])
        self.assertEqual(run.api_endpoint,endpoint);run.driver.click.assert_not_called()
        self.assertFalse(run.stopped)
        run.controller.call.side_effect=None;run.controller.call.return_value={'stopped':True,'identity':identity(99)}
        with self.assertRaises(ValueError):run.retire_applied_core(previous)

    def test_hot_apply_reobserves_existing_core_without_duplicate_registration(self):
        run,status,endpoint,graph=self.tun_connect_fixture()
        run.connect_phase('apply-ready')
        with patch.object(run,'wait_ready_status',return_value=status),patch.object(M.base,'regular',side_effect=lambda p,*a:p):
            run.observe_ready_core(timeout=10,register=False)
        self.assertEqual([c.args[0] for c in run.controller.call.call_args_list],['core_attest','core_attest'])
        run.rpc.assert_called_once_with('capabilities');run.driver.click.assert_not_called()

    def test_manual_policy_activation_precedes_new_stream_and_keeps_same_core_switch_assertion(self):
        run=M.TunCarrierRun.__new__(M.TunCarrierRun);run.core={'pid':10,'startTime':'1'};calls=[]
        run.boundary=Mock();run.record=Mock();run.exchange=Mock(return_value={'matched':True})
        run.fixture_control=Mock(return_value={'errors':[],'closed':False})
        run.pick=lambda label:calls.append(('pick',label))
        run.preference=lambda wanted:calls.append(('save',wanted))
        def activate(wanted):
            calls.append(('activate',wanted));run.core={'pid':11 if wanted else 10,'startTime':'2' if wanted else '1'}
        run.activate_saved_preference=activate
        def stream(label):
            calls.append(('socket',label,run.core['pid']));sock=Mock();sock.recv.return_value=b''
            return sock,{'backend':label}
        run.socket=stream;run.same_core=lambda baseline:calls.append(('same-core',baseline['pid']))
        with patch.object(M.base,'wait_for',side_effect=lambda read,**kwargs:read()):run.manual_cases()
        self.assertEqual([v for v in calls if v[0]=='socket'],[('socket','A',10),('socket','B',10),('socket','A',11),('socket','B',11)])
        self.assertLess(calls.index(('pick','A')),calls.index(('activate',False)))
        self.assertLess(calls.index(('activate',True)),calls.index(('socket','A',11)))
        self.assertIn(('same-core',11),calls)
        source=Path(M.__file__).read_text()
        auto=source.split('    def auto_case(',1)[1].split('    def ',1)[0]
        self.assertLess(auto.index('self.prepare_auto()'),auto.index('self.socket('))
        self.assertNotIn('auto-choice-pick',auto)
        for method in ('auto_case','uncovered_case'):
            body=source.split('    def '+method+'(',1)[1].split('    def ',1)[0]
            self.assertIn('self.activate_saved_preference(False)',body)
            self.assertLess(body.index('self.activate_saved_preference(False)'),body.index('self.socket('))

    def test_tun_connect_rejects_attestation_graph_and_listener_failures(self):
        for failure in ('root-runtime', 'tun-graph', 'owned-listener'):
            run, status, endpoint, graph = self.tun_connect_fixture()
            if failure == 'root-runtime': run.observed_api_endpoint.return_value = (endpoint, {'sha256':'b'*64}, graph)
            if failure == 'tun-graph': run.observed_api_endpoint.return_value = (endpoint, {'sha256':H}, {'inbounds':[]})
            if failure == 'owned-listener': run.rpc.side_effect = ValueError('root owned socket mismatch')
            with patch.object(run,'wait_ready_status',return_value=status), patch.object(M.base, 'regular', side_effect=lambda p,*args: p):
                with self.assertRaises(ValueError): run.connect()
            self.assertNotIn('execution', run.result)
            self.assertNotIn('apiEndpoint', run.result)
            self.assertEqual(run.result['connectObservation']['phase'], 'owned-api-probe' if failure == 'owned-listener' else 'runtime-api')

    def test_shared_activation_waits_for_tun_target_without_registration_or_mutation(self):
        self.assertIs(M.TunCarrierRun.activate_saved_graph, M.base.CarrierRun.activate_saved_graph)
        self.assertIs(M.TunCarrierRun.activation_runtime_ready, M.base.CarrierRun.activation_runtime_ready)
        run=M.TunCarrierRun.__new__(M.TunCarrierRun)
        run.result={};run.core={'pid':10,'startTime':'1'};run.api_endpoint={'port':1234}
        run.controller=Mock();run.status=Mock(return_value={'running':True,'pid':10,'mainGeneration':3})
        run.same_core=Mock();run.gui_phase=Mock();run.apply=Mock();run.driver=SimpleNamespace(session='private-session')
        run.pending_apply_control=Mock(return_value={'bars':0,'buttons':0,'disabled':False})
        old={'outbounds':[{'tag':'proxy-selector','type':'selector'}]}
        new={'outbounds':[{'tag':'proxy-selector','type':'selector','interrupt_exist_connections':True}]}
        proof={'pid':10,'startTime':'1','sha256':H}
        run.observed_api_endpoint=Mock(side_effect=[(run.api_endpoint,proof,old),(run.api_endpoint,proof,new),(run.api_endpoint,proof,new)])
        def wait(read,condition=bool,**kwargs):
            for _ in range(4):
                value=read()
                if condition(value):return value
            raise M.base.GuiFailure('observation-timeout')
        with patch.object(M.base,'wait_for',side_effect=wait):
            result=run.activate_saved_graph(True,lambda:{'value':True},run.preference_runtime,'preferenceActivations')
        self.assertEqual(result['apply'],'not-needed');self.assertEqual(run.result['activationObservation']['samples'],2)
        self.assertTrue(result['acceptedRuntime']['selectorInterrupt'])
        run.apply.assert_not_called();run.controller.call.assert_not_called()

    def test_tun_readiness_uses_shared_busy_deadline_without_root_or_start_replay(self):
        self.assertIs(M.TunCarrierRun.wait_ready_status,M.base.CarrierRun.wait_ready_status)
        self.assertIs(M.TunCarrierRun.connect_status,M.base.CarrierRun.connect_status)
        run=M.TunCarrierRun.__new__(M.TunCarrierRun);run.result={};run.connect_phase('stopped-check')
        run.driver=Mock();run.driver.session='private-session';run.controller=Mock()
        run.readiness_observation_fence=Mock()
        run.status=Mock(side_effect=[M.base.NativeRejected('busy','status-cache-busy'),{'running':True,'starting':False,'mainGeneration':2,'pid':20}])
        with patch.object(M.base.time,'sleep'):self.assertTrue(run.wait_ready_status(1)['running'])
        self.assertEqual(run.result['connectObservation']['savedStateObservation']['busyCount'],1)
        run.driver.click.assert_not_called();run.controller.call.assert_not_called()
        run.status=Mock(side_effect=M.base.NativeRejected('unknown','saved-state-unavailable'))
        with self.assertRaises(M.base.NativeRejected):run.wait_ready_status(1)
        self.assertEqual(run.status.call_count,1)

    def test_tun_rpc_brackets_root_call_without_supplying_endpoint_authority(self):
        run = M.TunCarrierRun.__new__(M.TunCarrierRun)
        run.controller = Mock(); run.revalidate_api_endpoint = Mock()
        run.controller.call.return_value = {'schema':'polaris-package-rpc-probe-v1', 'ok':True, 'action':'tun-capabilities', 'result':{}}
        run.rpc('capabilities')
        self.assertEqual(run.revalidate_api_endpoint.call_count,2)
        run.controller.call.assert_called_once_with('rpc', method='capabilities')
        run.revalidate_api_endpoint.side_effect = ValueError('status endpoint mismatch')
        run.controller.reset_mock()
        with self.assertRaises(ValueError): run.rpc('capabilities')
        run.controller.call.assert_not_called()


    def membership_recovery(self):
        return {'schema':'polaris-sudo-membership-recovery-v1','account':'runner','guiUid':1001,
                'group':'sudo','groupGid':27,'originalMembership':'absent','finalMembership':'absent',
                'removedOnlyAddedMembership':True,'writerStopped':True,'maxLeaseSeconds':90}

    def test_joint_membership_recovery_is_exact_bound_and_no_gid_assumption(self):
        recovery = self.membership_recovery()
        self.assertIs(M.verify_group_recovery(recovery,1001,'runner'), recovery)
        alternate = {**recovery,'groupGid':451}
        self.assertEqual(M.verify_group_recovery(alternate,1001,'runner',451)['groupGid'],451)
        for key,value in {'schema':'fake','account':'other','guiUid':0,'group':'admin','groupGid':True,
                          'originalMembership':'present','finalMembership':'present','removedOnlyAddedMembership':False,
                          'writerStopped':False,'maxLeaseSeconds':91}.items():
            with self.assertRaises(ValueError): M.verify_group_recovery({**recovery,key:value},1001,'runner')
        for args in ((recovery,1002,'runner'),(recovery,1001,'other'),(recovery,1001,'runner',451)):
            with self.assertRaises(ValueError): M.verify_group_recovery(*args)
        with self.assertRaises(ValueError): M.verify_group_recovery({**recovery,'secret':'private'},1001,'runner')

    def test_joint_group_lease_never_substitutes_for_real_password_relock(self):
        secret_free = {'guiUid':1001,'account':'runner','finalPasswordStatus':'L','plaintextPersisted':False,
                      'emptyPasswordRestored':False,'temporaryPasswordHash':'locked until disposable VM destruction'}
        proof = {'credentialRecovery':secret_free,'credentialAuthenticationComplete':True,'authDescendantsReaped':True,
                 'jointLease':{'credentialRecovery':dict(secret_free),'groupRecovery':self.membership_recovery()}}
        subject = {'guiUid':1001,'account':'runner'}
        self.assertEqual(M.verify_joint_recovery(proof,subject)['groupGid'],27)
        for key in ('credentialAuthenticationComplete','authDescendantsReaped'):
            with self.assertRaises(ValueError): M.verify_joint_recovery({**proof,key:False},subject)
        for key,value in {'finalPasswordStatus':'P','account':'root','guiUid':0,'plaintextPersisted':True,
                          'emptyPasswordRestored':True,'temporaryPasswordHash':'restored empty hash'}.items():
            bad = copy.deepcopy(proof); bad['credentialRecovery'][key]=value
            bad['jointLease']['credentialRecovery'][key]=value
            with self.assertRaises(ValueError): M.verify_joint_recovery(bad,subject)
        bad = copy.deepcopy(proof); bad['jointLease']['credentialRecovery']['account']='other'
        with self.assertRaises(ValueError): M.verify_joint_recovery(bad,subject)

    def test_group_preparation_is_only_guarded_pending_not_authentication(self):
        value = {'guarded':True,'authentication':'pending-real-product-authorization',
                 'authenticationReadiness':{'authenticated':False,'groupMembership':'pending-guarded-lease'}}
        self.assertEqual(M.verify_group_preparation(value),'prepared-pending-group-lease')
        for mutate in (lambda v:v.update(guarded=False),lambda v:v.update(authentication='ready'),
                       lambda v:v['authenticationReadiness'].update(authenticated=True),
                       lambda v:v['authenticationReadiness'].update(groupMembership='ready')):
            bad=copy.deepcopy(value);mutate(bad)
            with self.assertRaises(ValueError): M.verify_group_preparation(bad)


    def test_group_gui_adapter_cannot_close_window_on_unproved_joint_recovery(self):
        credential={'guiUid':1001,'account':'runner','finalPasswordStatus':'L','plaintextPersisted':False,
                    'emptyPasswordRestored':False,'temporaryPasswordHash':'locked until disposable VM destruction'}
        proof={'owned':[],'credentialRecovery':credential,'credentialAuthenticationComplete':True,'authDescendantsReaped':True,
               'jointLease':{'credentialRecovery':dict(credential),'groupRecovery':self.membership_recovery()}}
        for valid in (True,False):
            run=M.TunCarrierRun.__new__(M.TunCarrierRun); run.env={}; run.result={}
            run.app={'pid':100,'startTime':'123','uid':1001};run.driver=Mock();run.supervisor=Mock();run.controller=Mock()
            run.launch_options={'authenticationMode':M.AUTH_GROUP_MODE,'authorization':{'guiUid':1001,'account':'runner'}}
            result=copy.deepcopy(proof)
            if not valid: result['jointLease']['groupRecovery']['writerStopped']=False
            run.controller.call.side_effect=lambda command,**args:{**end_snapshot(run),**result} if command=='transaction_end' else scan_snapshot(run) if command=='transaction_scan' else begin_snapshot(run) if command=='transaction_begin' else {}
            run.text=lambda key:key
            replies=iter([[],[helper_row()]])
            run.tun_observations=lambda:next(replies)
            if valid:
                self.assertEqual(run.helper_action('install')['authorization'],{**end_snapshot(run),**result})
                self.assertEqual(run.membership_gid,27)
                run.supervisor.end_window.assert_called_once()
                self.assertEqual(run.supervisor.end_window.call_args.args[0]['transactionBinding'],run.transaction_binding)
            else:
                with self.assertRaises(ValueError):run.helper_action('install')
                run.supervisor.end_window.assert_not_called()



class SharedGuiAndVmContracts(unittest.TestCase):
    def test_tun_uses_shared_role_preference_without_button_assumption(self):
        run=M.TunCarrierRun.__new__(M.TunCarrierRun)
        with patch.object(M.base.CarrierRun,'preference',return_value='observed') as shared:
            self.assertEqual(run.preference(True),'observed');shared.assert_called_once_with(True)
        self.assertEqual(run.fixture_group_label,'Package isolated TUN')
        self.assertIs(M.TunCarrierRun.pick,M.base.CarrierRun.pick)
        source=Path(M.__file__).read_text()
        self.assertNotIn('button[role=switch]',source)
        self.assertIn('dialog[open][aria-labelledby="restart-apply-title"] button',source)

    def test_tun_exchange_retains_framed_fixture_and_pending_proof(self):
        run=M.TunCarrierRun.__new__(M.TunCarrierRun);run.env={};run.result={};run.pending_streams={}
        proof={'exit':'A','bytes':36,'nonceSha256':'a'*64}
        with patch.object(M.fixture,'exchange',return_value=proof) as exchange:
            self.assertEqual(run.exchange(Mock(),'A'),proof)
            exchange.assert_called_once()
        self.assertEqual(run.pending_streams['a'*64],proof)
        self.assertEqual(run.result['streamObservation']['exchanges'][0]['state'],'validated')

    def test_tun_failure_captures_shared_driver_evidence_before_recovery(self):
        source=Path(M.__file__).read_text()
        failure=source.index("'code': 'TUN_RUNTIME_FAILED'")
        capture=source.index('self.capture_driver_failure()', failure)
        recover=source.index('self.controller.recover()', failure)
        self.assertLess(capture, recover)

    def test_vm_preflight_error_is_validated_and_preserved_not_promoted(self):
        import root_controller
        for invalid in (False,True):
            client=M.Controller();client.closed=False
            observation=M.tun.vm_observation()
            if invalid:observation['secret']='PRIVATE'
            payload={'seq':1,'ok':False,'error':{'stage':'vm-preflight','code':'CONTROLLER_FAILED','vmPreflight':observation}}
            self.assertIn('vm-preflight',root_controller.CONTROLLER_STAGES)
            with patch.object(client,'_write'),patch.object(client,'_read',return_value=payload):
                with self.assertRaises(ValueError):client.call('prepare')
            saved=client.lifecycle['rootOperationFailure']
            if invalid:self.assertNotIn('vmPreflight',saved)
            else:self.assertEqual(saved['vmPreflight'],observation)
            self.assertNotIn('PRIVATE',json.dumps(client.lifecycle))

    def test_vm_abort_receipt_preserved_only_after_shared_validation(self):
        import root_controller
        for invalid in (False,True):
            observation=M.tun.vm_observation()
            if invalid:observation['failedProbe']='PRIVATE'
            value={'schema':'polaris-root-abort-v1','state':'failed','forced':True,'prepared':False,
                   'actualReadOnlyAuth':root_controller.auth_observation(),'unknown':[],'vmPreflight':observation}
            client=M.Controller();client.child=Mock();client.child.wait.return_value=1
            client.lifecycle.update(ready=True,spawned=True)
            with patch.object(client,'call',return_value=value):result=client.recover()
            self.assertEqual(result['state'],'failed')
            if invalid:self.assertNotIn('abortReceipt',client.lifecycle)
            else:self.assertEqual(client.lifecycle['abortReceipt']['vmPreflight'],observation)
            self.assertNotIn('PRIVATE',json.dumps(client.lifecycle))

class AuthenticationRuntimeConsumerTests(unittest.TestCase):
    def test_ipc_first_boundary_failure_survives_abort_without_raw_error(self):
        for stage in ('request-write','response-read'):
            client=M.Controller();client.closed=False
            with patch.object(client,'_write',side_effect=ValueError('PRIVATE') if stage=='request-write' else None), \
                 patch.object(client,'_read',side_effect=ValueError('PRIVATE')):
                with self.assertRaises(ValueError):client.call('transaction_begin',kind='install')
                with self.assertRaises(ValueError):client.call('abort')
            self.assertEqual(client.lifecycle['rootBoundaryFailure'],{'stage':stage,'action':'transaction_begin','code':'ROOT_IPC_UNCONFIRMED'})
            self.assertNotIn('PRIVATE',json.dumps(client.lifecycle))

    def test_wrong_sequence_error_is_not_current_operation_evidence(self):
        for seq in (None,True,0,2):
            client=M.Controller();client.closed=False
            value={'seq':seq,'ok':False,'error':{'stage':'dispatch','code':'CONTROLLER_FAILED'}}
            with patch.object(client,'_write'),patch.object(client,'_read',return_value=value):
                with self.assertRaises(ValueError):client.call('transaction_begin',kind='install')
            self.assertNotIn('rootOperationFailure',client.lifecycle)
            self.assertEqual(client.lifecycle['rootBoundaryFailure']['stage'],'response-envelope')

    def test_operation_error_strictly_preserves_safe_auth_runtime(self):
        import root_controller
        for invalid in (False,True):
            value=root_controller.auth_runtime_observation(root_controller.AUTH_GROUP_MODE)
            value['kind']='install';value['lastReached']='lineage-start'
            value['firstFailure']={'stage':'lineage-start','code':'operation-failed'}
            if invalid:value['secret']='PRIVATE'
            client=M.Controller();client.closed=False
            payload={'seq':1,'ok':False,'error':{'stage':'dispatch','code':'CONTROLLER_FAILED','authenticationRuntime':value}}
            with patch.object(client,'_write'),patch.object(client,'_read',return_value=payload):
                with self.assertRaises(ValueError):client.call('transaction_begin',kind='install')
            saved=client.lifecycle['rootOperationFailure']
            if invalid:self.assertNotIn('authenticationRuntime',saved)
            else:self.assertEqual(saved['authenticationRuntime'],value)
            self.assertNotIn('PRIVATE',json.dumps(client.lifecycle))

    def test_transaction_candidate_error_frame_keeps_only_validated_first_cause(self):
        import root_controller
        for code in ('transaction-candidate-unconfirmed','transaction-raw-secret'):
            observation=root_controller.auth_runtime_observation(root_controller.AUTH_GROUP_MODE)
            observation['kind']='install';observation['lastReached']='transaction-observer'
            observation['firstFailure']={'stage':'transaction-observer','code':code}
            client=M.Controller();client.closed=False
            payload={'seq':1,'ok':False,'error':{'stage':'dispatch','code':'CONTROLLER_FAILED',
                'authenticationRuntime':observation}}
            with patch.object(client,'_write'),patch.object(client,'_read',return_value=payload):
                with self.assertRaises(ValueError):client.call('transaction_scan',kind='install')
            saved=client.lifecycle['rootOperationFailure']
            if code=='transaction-candidate-unconfirmed':
                self.assertEqual(saved['authenticationRuntime'],observation)
                self.assertEqual(saved['authenticationRuntime']['firstFailure']['code'],code)
            else:
                self.assertNotIn('authenticationRuntime',saved)
                self.assertNotIn('transaction-raw-secret',json.dumps(client.lifecycle))

    def test_candidate_failure_real_producer_to_error_and_abort_consumer(self):
        import root_controller, threading, copy
        producer=root_controller.Controller.__new__(root_controller.Controller)
        producer.auth_runtime_lock=threading.RLock();producer.auth_checkpoint=Mock()
        producer.auth_runtime=root_controller.auth_runtime_observation(root_controller.AUTH_GROUP_MODE)
        producer.auth_runtime['kind']='install';producer.auth_runtime['lastReached']='transaction-observer'
        producer.auth_runtime['firstFailure']={'stage':'transaction-observer','code':'transaction-candidate-unconfirmed'}
        failure={'schema':'polaris-transaction-candidate-failure-v2','phase':'owned-parent',
                 'inventory':{'pid':61,'startTime':'1234','ppid':60},'relation':'owned-child',
                 'parent':{'pid':60,'startTime':'1234'},'outcome':'identity-mismatch','error':'contract',
                 'probe':'parent-identity','priorOwned':False}
        producer.auth_candidate_failure(failure)
        for fault in (None,'phase','probe','extra'):
            observation=copy.deepcopy(producer.auth_runtime)
            if fault=='phase':observation['candidateFailure']['phase']='PRIVATE'
            if fault=='probe':observation['candidateFailure']['probe']='PRIVATE'
            if fault=='extra':observation['candidateFailure']['raw']='PRIVATE'
            client=M.Controller();client.closed=False
            payload={'seq':1,'ok':False,'error':{'stage':'dispatch','code':'CONTROLLER_FAILED','authenticationRuntime':observation}}
            with patch.object(client,'_write'),patch.object(client,'_read',return_value=payload):
                with self.assertRaises(ValueError):client.call('transaction_scan',kind='install')
            saved=client.lifecycle['rootOperationFailure']
            if fault is None:self.assertEqual(saved['authenticationRuntime']['candidateFailure'],failure)
            else:self.assertNotIn('authenticationRuntime',saved)
            self.assertNotIn('PRIVATE',json.dumps(client.lifecycle))
        receipt={'schema':'polaris-root-abort-v1','state':'failed','forced':True,'prepared':True,
            'actualReadOnlyAuth':None,'unknown':['authentication-lease-recovery-unknown'],
            'authenticationRuntime':producer.auth_runtime,'productCleanup':{'state':'forced','forced':True,'unknown':[],
                'publicationEligible':False}}
        client=M.Controller();client.child=Mock();client.child.wait.return_value=1
        client.lifecycle.update(ready=True,spawned=True)
        with patch.object(client,'call',return_value=receipt):result=client.recover()
        self.assertEqual(result['state'],'failed')
        self.assertEqual(client.lifecycle['abortReceipt']['authenticationRuntime']['candidateFailure'],failure)

    def test_lineage_failure_real_producer_to_error_and_abort_consumer(self):
        import root_controller, threading, copy
        producer=root_controller.Controller.__new__(root_controller.Controller)
        producer.auth_runtime_lock=threading.RLock();producer.auth_checkpoint=Mock()
        producer.auth_runtime=root_controller.auth_runtime_observation(root_controller.AUTH_GROUP_MODE)
        producer.auth_runtime['kind']='install';producer.auth_runtime['lastReached']='transaction-observer'
        producer.auth_runtime['firstFailure']={'stage':'transaction-observer','code':'transaction-candidate-unconfirmed'}
        failure={'stage':'pre-stop','probe':'final-parent','error':'contract','priorOwned':True,
                 'process':{'pid':61,'startTime':'1234','ppid':60},'captureProbe':'stat-after','attempts':3}
        producer.auth_lineage_failure(failure)
        for fault in (None,'stage','probe','error','extra','priorOwned','process','captureProbe','attempts'):
            observation=copy.deepcopy(producer.auth_runtime)
            if fault is not None:observation['lineageFailure'][fault]='PRIVATE'
            client=M.Controller();client.closed=False
            payload={'seq':1,'ok':False,'error':{'stage':'dispatch','code':'CONTROLLER_FAILED','authenticationRuntime':observation}}
            with patch.object(client,'_write'),patch.object(client,'_read',return_value=payload):
                with self.assertRaises(ValueError):client.call('transaction_scan',kind='install')
            saved=client.lifecycle['rootOperationFailure']
            if fault is None:self.assertEqual(saved['authenticationRuntime']['lineageFailure'],failure)
            else:self.assertNotIn('authenticationRuntime',saved)
            self.assertNotIn('PRIVATE',json.dumps(client.lifecycle))
        receipt={'schema':'polaris-root-abort-v1','state':'failed','forced':True,'prepared':True,
            'actualReadOnlyAuth':None,'unknown':['authentication-lease-recovery-unknown'],
            'authenticationRuntime':producer.auth_runtime,'productCleanup':{'state':'forced','forced':True,'unknown':[],
                'publicationEligible':False}}
        client=M.Controller();client.child=Mock();client.child.wait.return_value=1
        client.lifecycle.update(ready=True,spawned=True)
        with patch.object(client,'call',return_value=receipt):result=client.recover()
        self.assertEqual(result['state'],'failed')
        self.assertEqual(client.lifecycle['abortReceipt']['authenticationRuntime']['lineageFailure'],failure)

    def test_ledger_failure_real_producer_to_error_and_abort_consumer(self):
        import root_controller, threading, copy
        producer=root_controller.Controller.__new__(root_controller.Controller)
        producer.auth_runtime_lock=threading.RLock();producer.auth_checkpoint=Mock()
        producer.auth_runtime=root_controller.auth_runtime_observation(root_controller.AUTH_GROUP_MODE)
        producer.auth_runtime['kind']='install';producer.auth_runtime['lastReached']='transaction-observer'
        producer.auth_runtime['firstFailure']={'stage':'transaction-observer','code':'transaction-candidate-unconfirmed'}
        error=PermissionError(13,'PRIVATE');error.ledger_operation='save';error.ledger_probe='file-fsync'
        producer.auth_ledger_failure(error)
        expected={'operation':'save','probe':'file-fsync','error':'eacces'}
        for fault in (None,'operation','probe','error','extra'):
            observation=copy.deepcopy(producer.auth_runtime)
            if fault is not None:observation['ledgerFailure'][fault]='PRIVATE'
            client=M.Controller();client.closed=False
            payload={'seq':1,'ok':False,'error':{'stage':'dispatch','code':'CONTROLLER_FAILED','authenticationRuntime':observation}}
            with patch.object(client,'_write'),patch.object(client,'_read',return_value=payload):
                with self.assertRaises(ValueError):client.call('transaction_scan',kind='install')
            saved=client.lifecycle['rootOperationFailure']
            if fault is None:self.assertEqual(saved['authenticationRuntime']['ledgerFailure'],expected)
            else:self.assertNotIn('authenticationRuntime',saved)
            self.assertNotIn('PRIVATE',json.dumps(client.lifecycle))
        receipt={'schema':'polaris-root-abort-v1','state':'failed','forced':True,'prepared':True,
            'actualReadOnlyAuth':None,'unknown':['authentication-lease-recovery-unknown'],
            'authenticationRuntime':producer.auth_runtime,'productCleanup':{'state':'forced','forced':True,'unknown':[],
                'publicationEligible':False}}
        client=M.Controller();client.child=Mock();client.child.wait.return_value=1
        client.lifecycle.update(ready=True,spawned=True)
        with patch.object(client,'call',return_value=receipt):result=client.recover()
        self.assertEqual(result['state'],'failed')
        self.assertEqual(client.lifecycle['abortReceipt']['authenticationRuntime']['ledgerFailure'],expected)

    def test_abort_keeps_safe_partial_proof_without_promoting_failure(self):
        import root_controller
        observation=root_controller.auth_runtime_observation(root_controller.AUTH_GROUP_MODE)
        observation['kind']='install';observation['lastReached']='password-writer'
        observation['firstFailure']={'stage':'password-writer','code':'operation-failed'}
        observation['recovery']['credential']='unknown'
        receipt={'schema':'polaris-root-abort-v1','state':'failed','forced':True,'prepared':True,
            'actualReadOnlyAuth':None,'unknown':['authentication-lease-recovery-unknown'],
            'authenticationRuntime':observation,'productCleanup':{'state':'forced','forced':True,'unknown':[],
                'publicationEligible':False}}
        client=M.Controller();client.child=Mock();client.child.wait.return_value=1
        client.lifecycle.update(ready=True,spawned=True)
        with patch.object(client,'call',return_value=receipt):result=client.recover()
        self.assertEqual(result['state'],'failed')
        self.assertEqual(client.lifecycle['abortReceipt']['authenticationRuntime'],observation)

    def test_helper_callback_failure_preserves_only_fixed_booleans(self):
        run=M.TunCarrierRun.__new__(M.TunCarrierRun);run.env={};run.result={};run.app={'pid':100,'startTime':'123','uid':1001}
        run.launch_options={'authenticationMode':'existing-session'}
        run.driver=Mock();run.supervisor=Mock();run.controller=Mock();run.text=lambda key:key
        run.controller.call.side_effect=lambda command,**kw:scan_snapshot(run) if command=='transaction_scan' else begin_snapshot(run) if command=='transaction_begin' else {}
        replies=iter([[],[helper_row(False,'installerFailed')]])
        run.tun_observations=lambda:next(replies)
        with self.assertRaises(ValueError):run.helper_action('install')
        self.assertEqual(run.result['guiObservation']['phase'],'helper-callback-observed')
        self.assertEqual({k:run.result['helperCallback'][k] for k in ('envelopeSuccess','success','installed','ready')},{'envelopeSuccess':True,'success':False,'installed':False,'ready':False})
        self.assertEqual(run.result['helperCallback']['errorCode'],'installerFailed')
        self.assertNotIn('PRIVATE',json.dumps(run.result))
        self.assertNotIn('transaction_end',[call.args[0] for call in run.controller.call.call_args_list])

class HelperObservationConsumerTests(unittest.TestCase):
    def run_fixture(self,row=None):
        run=M.TunCarrierRun.__new__(M.TunCarrierRun);run.env={};run.result={};run.app={'pid':100,'startTime':'123','uid':1001}
        run.launch_options={'authenticationMode':'existing-session'};run.driver=Mock();run.supervisor=Mock();run.controller=Mock();run.env={}
        run.text=lambda key:key
        run.controller.call.side_effect=lambda command,**kw:scan_snapshot(run) if command=='transaction_scan' else end_snapshot(run) if command=='transaction_end' else begin_snapshot(run) if command=='transaction_begin' else {}
        rows=iter([[],[helper_row() if row is None else row]])
        run.tun_observations=lambda:next(rows)
        return run

    def test_success_preserves_original_gate_and_observed_completion(self):
        run=self.run_fixture();out=run.helper_action('install')
        self.assertTrue(out['callback']['success']);self.assertEqual(out['callback']['association'],'shape-only')
        self.assertEqual(out['callback']['callbackObservedElapsedMs'],735)
        self.assertFalse(run.result['helperObservation']['commandIdentityClaimed'])
        self.assertNotIn('firstBusinessFailure',run.result)
        run.supervisor.end_window.assert_called_once()
        self.assertEqual(run.supervisor.end_window.call_args.args[0]['transactionBinding'],run.transaction_binding)

    def test_failure_cancel_partial_and_each_typed_code_stays_failure(self):
        for code in M.helper_observation.ERROR_CODES:
            run=self.run_fixture(helper_row(False,code))
            with self.subTest(code=code),self.assertRaises(ValueError):run.helper_action('install')
            self.assertEqual(run.result['firstBusinessFailure']['projection']['errorCode'],code)
            self.assertEqual(run.result['guiObservation']['phase'],'helper-callback-observed')
            self.assertEqual(run.result['rootTransactionObservation']['pkexecBirths'],[])
            run.supervisor.end_window.assert_not_called()
            self.assertNotIn('transaction_end',[c.args[0] for c in run.controller.call.call_args_list])

    def test_outer_failure_is_uncorrelated_not_business_failure(self):
        row=helper_row();p=row['projection'];p.update(kind='outerFailure',association='uncorrelated',envelopeSuccess=False,executionDiagnosticState='not-applicable',
            actionSuccess=None,errorCode='not-applicable',statusShape='not-applicable')
        p['status']={k:None for k in p['status']}
        run=self.run_fixture()
        self.assertIsNone(run.helper_observations('install',0,[row]))
        self.assertEqual(run.result['helperObservation']['rows'][0]['projection']['association'],'uncorrelated')
        self.assertNotIn('firstBusinessFailure',run.result)

    def test_unknown_malformed_projection_and_duplicate_never_pass(self):
        for change in (lambda r:r['projection'].update(schema='other'),lambda r:r['projection'].update(secret='PRIVATE'),
                       lambda r:r['projection'].update(errorCode='PRIVATE'),lambda r:r.update(observedElapsedMs=-1)):
            row=helper_row();change(row);run=self.run_fixture(row)
            with self.assertRaises(ValueError):run.helper_action('install')
            run.supervisor.end_window.assert_not_called();self.assertNotIn('PRIVATE',json.dumps(run.result))
        run=self.run_fixture();row=helper_row();other=copy.deepcopy(row);other['sequence']=2
        with self.assertRaises(ValueError):run.helper_observations('install',0,[row,other])

    def test_contradictory_success_or_unknown_fields_cannot_become_acceptance(self):
        for change in (lambda r:r['projection'].update(errorCode='cancelled'),
                       lambda r:r['projection'].update(errorCode='unknown'),
                       lambda r:r['projection']['status'].update(supported=False),
                       lambda r:r['projection'].update(statusShape='malformed')):
            row=helper_row();change(row);run=self.run_fixture(row)
            with self.assertRaises(ValueError):run.helper_action('install')
            run.supervisor.end_window.assert_not_called()
            self.assertNotIn('transaction_end',[c.args[0] for c in run.controller.call.call_args_list])

    def test_scan_failure_and_end_failure_have_distinct_fixed_phase(self):
        for command,phase in (('transaction_scan','helper-transaction-scan-failed'),('transaction_end','helper-transaction-end')):
            run=self.run_fixture();prior=run.controller.call.side_effect
            def call(action,**kw):
                if action==command:raise ValueError('PRIVATE')
                return prior(action,**kw)
            run.controller.call.side_effect=call
            with self.assertRaises(ValueError):run.helper_action('install')
            self.assertEqual(run.result['guiObservation']['phase'],phase)
            run.supervisor.end_window.assert_not_called();self.assertNotIn('PRIVATE',json.dumps(run.result))

    def test_scan_failure_still_preserves_already_delivered_business_failure(self):
        run=self.run_fixture(helper_row(False,'authorizationIncomplete'));prior=run.controller.call.side_effect
        original=ValueError('PRIVATE')
        def call(command,**kw):
            if command=='transaction_scan':raise original
            return prior(command,**kw)
        run.controller.call.side_effect=call
        with self.assertRaises(ValueError) as failed:run.helper_action('install')
        self.assertIs(failed.exception,original)
        self.assertEqual(run.result['firstBusinessFailure']['projection']['errorCode'],'authorizationIncomplete')
        self.assertEqual(run.result['helperObservationFailure']['stage'],'root-transaction-scan')
        self.assertNotIn('PRIVATE',json.dumps(run.result));run.supervisor.end_window.assert_not_called()

    def test_unclassified_callback_is_type_only_not_authorization(self):
        run=self.run_fixture()
        for name in ('null','array','object','string','number','boolean','undefined','other'):
            row={'sequence':1,'kind':'unclassifiedCallback','observedElapsedMs':2,'valueType':name}
            self.assertIsNone(run.helper_observations('install',0,[row]))
            self.assertEqual(run.result['helperObservation']['rows'][0]['association'],'uncorrelated')
            self.assertNotIn('firstBusinessFailure',run.result)
        row['valueType']='PRIVATE'
        with self.assertRaises(ValueError):run.helper_observations('install',0,[row])
        self.assertNotIn('PRIVATE',json.dumps(run.result))

    def test_root_snapshot_is_diagnostic_only_exact_subject_and_bounded_births(self):
        run=self.run_fixture();run.gui_phase('helper-transaction-scan');value=scan_snapshot(run)
        value.update(pkexecBirths=[[200,'456']],owned=[{'pid':200,'startTime':'456','uid':0,'exe':'PRIVATE'}])
        run.record_transaction_observation(value,'install')
        self.assertEqual(run.result['rootTransactionObservation']['pkexecBirths'],[[200,'456']])
        self.assertNotIn('PRIVATE',json.dumps(run.result));run.supervisor.end_window.assert_not_called()
        for change in (lambda v:v.update(kind='uninstall'),lambda v:v['subject'].update(pid=101),
                       lambda v:v.update(authorizationObserved=True),lambda v:v.update(raw='PRIVATE')):
            bad=copy.deepcopy(value);change(bad)
            with self.assertRaises(ValueError):run.record_transaction_observation(bad,'install')

    @unittest.skipUnless(shutil.which('node'), 'pure JS interpreter unavailable')
    def test_actual_observer_preserves_sealed_invoke_original_callback_return_and_throw(self):
        source = r"""
const callbacks=new Map();let received=null;const originalInvoke=()=>{};
globalThis.window={__TAURI_INTERNALS__:{}};
Object.defineProperty(window.__TAURI_INTERNALS__,'invoke',{value:originalInvoke,writable:false,configurable:false});
Object.defineProperty(window.__TAURI_INTERNALS__,'callbacks',{value:callbacks,writable:false,configurable:false});
callbacks.set(1,function(raw){received=raw;return 73;});
const thrown=new Error('PRIVATE');callbacks.set(2,()=>{throw thrown;});
new Function(SCRIPT)();window.__tunObservation.context='install';
const raw={success:true,data:{success:false,errorCode:'cancelled',diagnostic:'PRIVATE',status:{supported:true,installed:false,ready:false}}};
const returned=callbacks.get(1)(raw);let exactThrow=false;
try{callbacks.get(2)('PRIVATE');}catch(e){exactThrow=e===thrown;}
callbacks.get(1)({success:false,error:'PRIVATE'});
console.log(JSON.stringify({invokeUnchanged:window.__TAURI_INTERNALS__.invoke===originalInvoke,returned,exactThrow,
 firstCode:window.__tunObservation.firstHelperFailure.projection.errorCode,
 rows:window.__tunObservation.rows,originalReceived:received.success===false}));
""".replace('SCRIPT',json.dumps(M.TUN_OBSERVER))
        result=subprocess.run(['node','-e',source],capture_output=True,text=True,timeout=10,check=True)
        value=json.loads(result.stdout)
        self.assertTrue(value['invokeUnchanged']);self.assertTrue(value['exactThrow']);self.assertEqual(value['returned'],73)
        self.assertEqual(value['firstCode'],'cancelled');self.assertTrue(value['originalReceived'])
        self.assertEqual(value['rows'][1]['kind'],'unclassifiedCallback')
        self.assertEqual(value['rows'][1]['valueType'],'string')
        self.assertEqual(value['rows'][2]['projection']['association'],'uncorrelated')
        self.assertNotIn('PRIVATE',result.stdout)

    def test_first_business_failure_is_not_overwritten_by_later_cleanup_context(self):
        run=self.run_fixture();run.helper_observations('install',0,[helper_row(False,'cancelled')])
        first=copy.deepcopy(run.result['firstBusinessFailure'])
        later=helper_row(False,'partialCleanup');later['sequence']=2
        run.helper_observations('uninstall',1,[later])
        self.assertEqual(run.result['firstBusinessFailure'],first)


class AuthenticationAndExecutionConsumerTests(unittest.TestCase):
    def test_root_execution_is_preserved_but_not_authority(self):
        run=HelperObservationConsumerTests().run_fixture();run.gui_phase('helper-begin');value=scan_snapshot(run)
        run.record_transaction_observation(value,'install')
        saved=run.result['rootTransactionObservation']['authenticationRuntime']
        self.assertEqual(saved['schema'],'polaris-auth-runtime-observation-v2')
        self.assertFalse(run.result['rootTransactionObservation']['authorizationObserved'])
        value['authenticationRuntime']['execution']['agent']['exitCode']=127
        self.assertIsNone(saved['execution']['agent']['exitCode'])
        with self.assertRaises(ValueError):run.record_transaction_observation(value,'install')
    def test_native_marker_preserved_without_business_success(self):
        row=helper_row(False,'authorizationIncomplete')
        p=row['projection'];p['executionDiagnosticState']='valid'
        p['executionDiagnostic']={'schema':M.helper_observation.EXECUTION_SCHEMA,'exitCode':127,
            'marker':'noAuthenticationAgent','scope':M.helper_observation.EXECUTION_SCOPE}
        run=HelperObservationConsumerTests().run_fixture(row)
        with self.assertRaises(ValueError):run.helper_action('install')
        self.assertEqual(run.result['firstBusinessFailure']['projection']['executionDiagnostic']['exitCode'],127)
        self.assertFalse(run.result['helperCallback']['success'])
        self.assertNotIn('transaction_end',[c.args[0] for c in run.controller.call.call_args_list])
    def test_actual_installer_projector_to_consumer_preserves_safe_failure_only(self):
        failure={'schema':'polaris-helper-installer-failure-v1','stage':'manager-property','recovery':'recovery-unconfirmed'}
        diagnostic={'schema':M.helper_observation.EXECUTION_SCHEMA,'exitCode':1,'marker':'unknown',
                    'scope':M.helper_observation.EXECUTION_SCOPE,'installerFailure':failure}
        raw={'success':True,'data':{'success':False,'errorCode':'installerFailed','message':'PRIVATE',
             'status':{'supported':True,'installed':False,'ready':False},'executionDiagnostic':diagnostic}}
        for fault in (None,'stage','extra','null','exit-zero'):
            payload=copy.deepcopy(raw)
            if fault=='stage':payload['data']['executionDiagnostic']['installerFailure']['stage']='PRIVATE'
            elif fault=='extra':payload['data']['executionDiagnostic']['installerFailure']['raw']='PRIVATE'
            elif fault=='null':payload['data']['executionDiagnostic']['installerFailure']=None
            elif fault=='exit-zero':payload['data']['executionDiagnostic']['exitCode']=0
            script='const project='+M.helper_observation.PROJECTOR_JS+';console.log(JSON.stringify(project('+json.dumps(payload)+')));'
            projection=json.loads(subprocess.check_output(['node','-e',script],text=True))
            row=helper_row(False,'installerFailed');row['projection']=projection
            run=HelperObservationConsumerTests().run_fixture(row)
            with self.assertRaises(ValueError):run.helper_action('install')
            observed=run.result['firstBusinessFailure']['projection']
            self.assertNotIn('PRIVATE',json.dumps(run.result))
            self.assertNotIn('transaction_end',[c.args[0] for c in run.controller.call.call_args_list])
            if fault is None:
                self.assertEqual(observed['executionDiagnostic']['installerFailure'],failure)
                self.assertEqual(observed['association'],'shape-only')
                self.assertFalse(run.result['helperCallback']['success'])
                projection['executionDiagnostic']['installerFailure']['stage']='PRIVATE'
                self.assertEqual(observed['executionDiagnostic']['installerFailure'],failure)
            else:
                self.assertEqual(observed['executionDiagnosticState'],'malformed')
                self.assertIsNone(observed['executionDiagnostic'])

    def test_contradictory_success_with_execution_diagnostic_is_not_success(self):
        row=helper_row();row['projection']['executionDiagnosticState']='malformed'
        run=HelperObservationConsumerTests().run_fixture(row)
        with self.assertRaises(ValueError):run.helper_action('install')
        run.supervisor.end_window.assert_not_called()


class LogindReadOnlyObservationTests(unittest.TestCase):
    def test_unique_owner_exact_subject_and_bounded_calls(self):
        app=identity();supervisor=Mock();supervisor.query.side_effect=["(':1.27',)","(objectpath '/org/freedesktop/login1/session/c1',)"]
        with patch.object(M.base,'process_identity',return_value=app): result=M.observe_logind_session(supervisor,{},app)
        self.assertEqual(result['state'],'present');self.assertTrue(result['birthStable'])
        calls=supervisor.query.call_args_list;self.assertEqual(len(calls),2)
        self.assertIn(':1.27',calls[1].args[0]);self.assertNotIn('org.freedesktop.login1',calls[1].args[0])
        self.assertEqual(calls[1].args[0][-1],str(app['pid']))
        self.assertEqual(calls[0].kwargs['timeout'],2);self.assertNotIn('c1',json.dumps(result))
    def test_missing_owner_error_bad_reply_and_birth_change_stay_unknown(self):
        app=identity()
        for replies in ([ValueError('PRIVATE')],["('org.freedesktop.login1',)"],
                        ["(':1.2',)",ValueError('NoSessionForPID PRIVATE')],
                        ["(':1.2',)","PRIVATE"]):
            supervisor=Mock();supervisor.query.side_effect=replies
            with patch.object(M.base,'process_identity',return_value=app): result=M.observe_logind_session(supervisor,{},app)
            self.assertEqual(result['state'],'unknown');self.assertNotIn('PRIVATE',json.dumps(result))
        supervisor=Mock();supervisor.query.side_effect=["(':1.2',)","(objectpath '/org/freedesktop/login1/session/c1',)"]
        with patch.object(M.base,'process_identity',side_effect=[app,{**app,'startTime':'999'}]):result=M.observe_logind_session(supervisor,{},app)
        self.assertEqual(result['state'],'unknown');self.assertFalse(result['birthStable'])
    def test_unknown_does_not_erase_supervisor_cleanup_evidence(self):
        app=identity();supervisor=Mock();supervisor.unknown=['tool-query-timeout'];supervisor.query.side_effect=ValueError('PRIVATE')
        with patch.object(M.base,'process_identity',return_value=app):M.observe_logind_session(supervisor,{},app)
        self.assertEqual(supervisor.unknown,['tool-query-timeout'])
    def test_session_probe_precedes_lease_and_never_calls_mutation(self):
        source=__import__('inspect').getsource(M.TunCarrierRun.helper_action)
        self.assertLess(source.index('observe_logind_session'),source.index("self.controller.call('transaction_begin'"))
        probe=__import__('inspect').getsource(M.observe_logind_session)
        self.assertNotIn('CreateSession',probe);self.assertNotIn('ActivateSession',probe)


class ExplicitStatusWaitConsumerTests(unittest.TestCase):
    def fixture(self):
        run=M.TunCarrierRun.__new__(M.TunCarrierRun);run.result={};run.core=identity(10)
        run.driver=SimpleNamespace(session='private-session',click=Mock())
        run.readiness_observation_fence=Mock();run.gui_phase=Mock();run.controller=Mock();run.stopped=False
        return run

    def test_off_busy_uses_shared_wait_once_then_exact_root_ledger_exit(self):
        self.assertIs(M.TunCarrierRun.wait_status_observation,M.base.CarrierRun.wait_status_observation)
        run=self.fixture();run.status=Mock(side_effect=[{'running':True},
            M.base.NativeRejected('PRIVATE','status-writer-busy'),{'running':False,'starting':False}])
        run.controller.call.return_value={'stopped':True,'identity':run.core}
        with patch.object(M.base.time,'sleep'):proof=run.disconnect()
        self.assertTrue(proof['stopped']);self.assertTrue(run.stopped)
        run.driver.click.assert_called_once_with('#connect-btn');run.controller.call.assert_called_once_with('core_stopped')
        self.assertEqual(run.result['statusWaitObservations'][0]['savedStateObservation']['busyCount'],1)
        # Still require exact original root-ledger birth after the read completes.
        run=self.fixture();run.status=Mock(side_effect=[{'running':True},{'running':False}])
        run.controller.call.return_value={'stopped':True,'identity':identity(99)}
        with self.assertRaises(ValueError):run.disconnect()
        self.assertFalse(run.stopped)

    def test_off_hard_status_error_does_not_replay_click_or_claim_root_exit(self):
        for diagnostic in ('status-cache-cold','status-writer-poisoned','unknown'):
            run=self.fixture();run.status=Mock(side_effect=[{'running':True},M.base.NativeRejected('PRIVATE',diagnostic)])
            with self.subTest(diagnostic=diagnostic),self.assertRaises(M.base.NativeRejected):run.disconnect()
            run.driver.click.assert_called_once_with('#connect-btn');run.controller.call.assert_not_called()
            self.assertFalse(run.stopped)

    def test_auto_loop_uses_one_original_timer_and_positive_sample_not_redundant_read(self):
        run=self.fixture();run.boundary=Mock();run.prepare_auto=Mock();run.network=Mock();run.same_core=Mock()
        run.preference=Mock();run.activate_saved_preference=Mock()
        run.socket=Mock(side_effect=[('old',{'backend':'A'}),('new',{'backend':'B'})]);run.exchange=Mock(return_value={'backend':'A'})
        run.config=Mock(return_value={'selectionIntent':{'mode':'auto'}});run.fixture_control=Mock(return_value={'errors':[],'closed':False})
        run.record=Mock();run.snapshot=Mock(return_value={})
        run.status=Mock(side_effect=[M.base.NativeRejected('PRIVATE','status-cache-busy'),{'actualServerId':'node-b'}])
        with patch.object(M.time,'sleep') as sleep:run.auto_case()
        self.assertEqual(run.status.call_count,2);self.assertEqual(run.exchange.call_count,3)
        self.assertEqual(run.socket.call_count,2);sleep.assert_called_once_with(3)
        run.same_core.assert_called_once_with(run.core,status={'actualServerId':'node-b'})
        self.assertEqual(run.result['statusWaitObservations'][0]['context'],'auto')
        run.driver.click.assert_not_called()

    def test_ready_binding_busy_reacquires_root_proof_but_registers_only_once(self):
        run,status,endpoint,graph=ControllerLifecycleContracts().tun_connect_fixture()
        run.readiness_observation_fence=Mock();run.driver.session='same-session'
        run.observed_api_endpoint.side_effect=[M.base.NativeRejected('PRIVATE','status-writer-busy'),(endpoint,{'sha256':H},graph)]
        with patch.object(run,'wait_ready_status',return_value=status),patch.object(M.base,'regular',side_effect=lambda p,*a:p),patch.object(M.time,'sleep'):
            run.connect()
        actions=[c.args[0] for c in run.controller.call.call_args_list]
        self.assertEqual(actions,['register_core','core_attest','core_attest','core_attest','tun_register'])
        run.driver.click.assert_called_once_with('#connect-btn')
        self.assertEqual(run.result['statusWaitObservations'][0]['savedStateObservation']['busyCount'],1)

    def test_explicit_status_sample_does_not_replace_tun_root_attestation(self):
        run=self.fixture();run.controller.call.return_value={'identity':identity(99)}
        run.status=Mock()
        with self.assertRaises(ValueError):run.same_core(run.core,status={'pid':10})
        run.controller.call.assert_called_once_with('core_attest');run.status.assert_not_called()



class AuxiliaryTerminalConsumerTests(unittest.TestCase):
    def fixture(self):
        obj=supervisor();gui=identity(10,1001,9);obj.begin_window('install',gui)
        strong=[{**identity(20,0,10),'capEff':0,'capAmb':0},{**identity(21,0,20),'capEff':0,'capAmb':0}]
        proof=bind_supervisor_proof(obj,strong)
        inv={'pid':22,'startTime':'1234','ppid':21}
        row={'schema':'polaris-unverified-auxiliary-terminal-v1','binding':copy.deepcopy(proof['transactionBinding']),
             'inventory':inv,'parent':{'pid':21,'startTime':'1234'},'anchor':{'pid':20,'startTime':'1234'},
             'capture':{'phase':'inputs-before','probe':'cmdline-empty','error':'contract'},
             'terminal':{'pid':22,'startTime':'1234','state':'absent','scope':'sampled-terminal-unverified-execution'},
             'priorStrongOwned':False,'identityVerified':False,'signalAuthority':False}
        proof.update(auxiliaryIdentityCoverage='incomplete',auxiliaryTerminals=[row])
        obj.quarantine[(22,'1234')]={**inv,'uid':0,'state':'S'};obj.window['births'].add((22,'1234'))
        return obj,proof,row

    def test_terminal_class_never_enters_owned_strong_confirmation_or_signal_authority(self):
        obj,proof,row=self.fixture();obj.unknown.append('previous-security-failure')
        with patch.object(M.tun,'unverified_terminal_identity',return_value=row['terminal']):obj.end_window(proof)
        self.assertIsNone(obj.window);self.assertNotIn((22,'1234'),obj.confirmed)
        self.assertIn((22,'1234'),obj.auxiliary_confirmed);self.assertIn((22,'1234'),obj.quarantine)
        self.assertNotIn(22,obj.owned);self.assertEqual(obj.unknown,['previous-security-failure'])
        with patch.object(M.os,'getuid',return_value=1001),patch.object(M.base.Supervisor,'signal') as signal:
            with self.assertRaises(ValueError):obj.signal(obj.quarantine[(22,'1234')],15)
            signal.assert_not_called()

    def test_nonce_kind_subject_and_execution_mismatch_reject_window_without_confirmation(self):
        for field in ('windowNonce','kind','subject','sourceSha','runToken'):
            obj,proof,row=self.fixture();bad=copy.deepcopy(proof)
            bad['transactionBinding'][field]={'pid':99,'startTime':'1234','uid':1001} if field=='subject' else 'uninstall' if field=='kind' else 'd'*len(str(bad['transactionBinding'][field]))
            with self.subTest(field=field),self.assertRaises(ValueError):obj.end_window(bad)
            self.assertIsNotNone(obj.window);self.assertEqual(obj.confirmed,set());self.assertEqual(obj.auxiliary_confirmed,{})

    def test_unknown_live_changed_parent_mandatory_overlap_and_extra_fields_fail(self):
        for fault in ('live','permission','parent','outside','overlap','raw','mandatory','coverage','reappeared'):
            obj,proof,row=self.fixture()
            if fault=='parent':obj.quarantine[(22,'1234')]['ppid']=99
            if fault=='outside':obj.window['births'].clear()
            if fault=='overlap':proof['owned'].append({**row['inventory'],'uid':0})
            if fault=='raw':row['raw']='PRIVATE'
            if fault=='mandatory':proof['rootShellBirths']=[]
            if fault=='coverage':proof['auxiliaryIdentityCoverage']='complete'
            response={**row['terminal'],'state':'zombie' if fault=='reappeared' else 'absent'}
            error=ValueError('live or reused') if fault=='live' else PermissionError() if fault=='permission' else None
            with self.subTest(fault=fault),patch.object(M.tun,'unverified_terminal_identity',side_effect=error,return_value=response),self.assertRaises((ValueError,PermissionError)):
                obj.end_window(proof)
            self.assertIsNotNone(obj.window);self.assertEqual(obj.auxiliary_confirmed,{})

    def test_commit_rechecks_new_quarantine_birth_parent_under_lock(self):
        obj,proof,row=self.fixture();obj.quarantine.clear();obj.window['births'].clear()
        def race(_):
            obj.quarantine[(22,'1234')]={**row['inventory'],'ppid':99,'uid':0}
            obj.window['births'].add((22,'1234'))
            return row['terminal']
        with patch.object(M.tun,'unverified_terminal_identity',side_effect=race),self.assertRaises(ValueError):obj.end_window(proof)
        self.assertIsNotNone(obj.window);self.assertFalse(obj.auxiliary_confirmed)

    def test_cleanup_requires_whole_pid_absence_not_zombie_or_unreadable(self):
        for terminal in ('absent','zombie','permission'):
            obj,proof,row=self.fixture()
            with patch.object(M.tun,'unverified_terminal_identity',return_value=row['terminal']):obj.end_window(proof)
            with patch.object(M.base,'process_identity',return_value=None),patch.object(M.base.Supervisor,'close',return_value={'state':'normal','unknown':[]}),\
                 patch.object(M.tun,'unverified_terminal_identity',side_effect=PermissionError() if terminal=='permission' else None,
                              return_value={**row['terminal'],'state':terminal}):result=obj.close(True)
            self.assertEqual(result['auxiliaryIdentityCoverage'],'incomplete')
            self.assertEqual(result['state'],'normal' if terminal=='absent' else 'failed')
            self.assertEqual(result['unknown'],[] if terminal=='absent' else ['auxiliary-terminal-not-reaped'])

    def test_observation_projection_preserves_incomplete_without_promoting_ownership(self):
        run=HelperObservationConsumerTests().run_fixture();run._gui_started=M.time.monotonic()
        obj,proof,row=self.fixture()
        # Bind this producer-shaped observation to this exact consumer subject/run.
        run.app=dict(proof['subject']);run.transaction_launch={'runId':'a'*12,'execution':{'sourceSha':'b'*40,'githubRunId':'1','githubRunAttempt':'1'}}
        run.transaction_binding=copy.deepcopy(proof['transactionBinding'])
        value=scan_snapshot(run);value.update({k:copy.deepcopy(proof[k]) for k in ('transactionBinding','owned','pkexecBirths','rootShellBirths','authorizationObserved','auxiliaryIdentityCoverage','auxiliaryTerminals')})
        run.record_transaction_observation(value,'install')
        self.assertEqual(run.result['auxiliaryIdentityCoverage'],'incomplete')
        observed=run.result['rootTransactionObservation']
        self.assertNotIn([22,'1234'],observed['ownedBirths']);self.assertEqual(observed['auxiliaryTerminals'],[row])
        value['auxiliaryTerminals']=[];value['auxiliaryIdentityCoverage']='complete'
        run.record_transaction_observation(value,'install')
        self.assertEqual(run.result['auxiliaryIdentityCoverage'],'incomplete')

    def test_real_ordinary_orphan_grandchild_is_not_cleared_by_terminal_class(self):
        # Isolated ordinary Python subreaper only; no product/root/network/actions.
        script=r'''
import ctypes,importlib.util,json,os,sys,threading,time
sys.path.insert(0,os.path.dirname(sys.argv[1]))
spec=importlib.util.spec_from_file_location('plain_tun',sys.argv[1]);m=importlib.util.module_from_spec(spec);spec.loader.exec_module(m)
assert ctypes.CDLL(None).prctl(36,1,0,0,0)==0
info_r,info_w=os.pipe();release_r,release_w=os.pipe()
child=os.fork()
if child==0:
 os.close(info_r);os.close(release_w)
 grandchild=os.fork()
 if grandchild==0:
  os.close(info_w);os.read(release_r,1);os._exit(0)
 os.write(info_w,str(grandchild).encode());os._exit(0)
os.close(info_w);os.close(release_r);pid=int(os.read(info_r,64));os.close(info_r);os.waitpid(child,0)
try:
 current=m.base.process_identity(pid);assert current and current['ppid']==os.getpid()
 obj=m.TunSupervisor.__new__(m.TunSupervisor)
 obj.lock=threading.RLock();obj.window=None;obj.quarantine={};obj.confirmed=set();obj.auxiliary_confirmed={}
 obj.controller_birth=None;obj.controller_descendants={};obj.gui_birth=None
 obj.owned={};obj.children={};obj.roles={};obj.unknown=[];obj.forced=[]
 obj.diagnostics=[];obj.diagnostics_truncated=False;obj.diagnostic_executable_budget=[m.base.DIAGNOSTIC_EXE_LIMIT]
 obj.scan()
 assert 'unobserved-adopted-descendant' in obj.unknown
 assert obj.roles.get(pid)=='unknown'
 print(json.dumps({'adopted':True,'unknownRetained':True,'signalUsed':False}))
finally:
 os.write(release_w,b'x');os.close(release_w);os.waitpid(pid,0)
'''
        result=subprocess.run([sys.executable,'-B','-c',script,str(Path(M.__file__))],text=True,capture_output=True,timeout=10)
        self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(json.loads(result.stdout),{'adopted':True,'unknownRetained':True,'signalUsed':False})


if __name__ == '__main__':
    unittest.main()
