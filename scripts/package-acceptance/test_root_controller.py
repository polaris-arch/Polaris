"""Pure/mock controller contracts; never execute root/product/account/network tools."""
import copy
import base64
import builtins
import stat
import importlib.util
import io
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import threading
import time
import types
import unittest
from unittest.mock import Mock, patch

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location('test_root_controller_module', HERE / 'root_controller.py')
m = importlib.util.module_from_spec(spec); spec.loader.exec_module(m)
spec_tun = importlib.util.spec_from_file_location('vm_test_tun', HERE / 'tun.py')
vm_tun = importlib.util.module_from_spec(spec_tun); spec_tun.loader.exec_module(vm_tun)
H = 'a' * 64


def launch(mode='existing-session'):
    now = int(time.time())
    execution = {'repository': 'owner/Polaris', 'sourceSha': 'b' * 40, 'sourceTree': 'c' * 40,
                 'githubRunId': '12345', 'githubRunAttempt': '1', 'actor': 'owner',
                 'workflowRef': 'owner/Polaris/.github/workflows/package.yml@refs/heads/reviewed', 'eventSha256': H}
    value = {'schema': m.SCHEMA, 'runId': '0123456789ab', 'guiUid': 1001,
             'driver': {'pid': 20, 'startTime': '1234'}, 'guiHome': '/tmp/qa/home',
             'helperScriptDir': '/tmp/qa/home/config/polaris', 'runtimePath': '/tmp/qa/home/config/polaris/singbox-runtime.json',
             'appSha256': H, 'helperSha256': H, 'coreSha256': H, 'carrierSha256': H,
             'kernelCommit': 'c6c0d8b3b83ab4d6e7b953dd2dbdb7efe6f8021a', 'originalAdmission': 'verified',
             'vmContext': {'githubHosted': True, 'dedicatedVM': True}, 'materialsDownloaded': True,
             'codeHashes': dict.fromkeys(m.FILES, H), 'authenticationMode': mode, 'execution': execution,
             'authorization': None, 'rpcProbePath': '/tmp/admitted-test-probe', 'rpcProbeSha256': H}
    if mode == m.AUTH_MODE:
        value['authorization'] = {'schema': 'polaris-tun-credential-lease-v1', 'phrase': m.AUTH_PHRASE,
            'authMode': m.AUTH_MODE, 'execution': copy.deepcopy(execution), 'runToken': value['runId'],
            'guiUid': value['guiUid'], 'account': 'runner', 'transactions': ['install', 'uninstall'],
            'notBeforeUnix': now - 1, 'expiresUnix': now + 899, 'maxLeaseSeconds': 90}
    return value


def identity(pid=50, uid=1001):
    return {'pid': pid, 'startTime': '2345', 'state': 'S', 'ppid': 20, 'uid': uid,
            'exeDev': 1, 'exeInode': 77, 'exe': '/usr/bin/test-app', 'capEff': 0, 'capAmb': 0,
            'namespaces': {'net': 1, 'mnt': 2, 'pid': 3, 'cgroup': 4}, 'cgroup': '0::/test.slice'}


class Ledger:
    def __init__(self):
        self.value = {'phase': 'guarded', 'result': 'incomplete', 'forcedCleanup': False, 'unknown': [],
                      'processes': [], 'transactionProcesses': [], 'deadlineMonotonic': time.monotonic() + 600,
                      'authenticationWatchdogActive': True,
                      'authentication': {'accountMutated': False, 'closed': False, 'agent': None}}
        self.children = {}; self.ops = Mock(); self.ops.proc.return_value = None
        self.directory = Path('/root/not-executed'); self.run_id = '0123456789ab'
    def load(self): return copy.deepcopy(self.value)
    def mutate(self, change): change(self.value); return self.load()


def controller(mode='existing-session'):
    tun = Mock(); tun.Commands.return_value = Mock()
    for name in ('vm_observation', 'validate_vm_observation', 'vm_error'):
        setattr(tun, name, getattr(vm_tun, name))
    tun.same_process.side_effect = lambda a, b: bool(a and b and all(a.get(k) == b.get(k) for k in ('pid','startTime','uid','exeDev','exeInode','namespaces','cgroup')))
    c = m.Controller(launch(mode), tun, Path('/root/not-executed'))
    c.ledger = Ledger(); c.ops = c.ledger.ops
    c.gui = identity(); return c


class AbortLeaseRegressionTests(unittest.TestCase):
    def test_existing_abort_fail_path_closes_lease_before_auth_and_product(self):
        c = controller(m.AUTH_MODE); order = []
        c.ledger.value['authentication']['accountMutated'] = True
        c.lease = Mock()
        def close_lease():
            order.append('lease')
            c.ledger.value['authentication']['accountMutated'] = False
        c.lease.close.side_effect = close_lease
        def auth(*args, **kwargs):
            self.assertFalse(c.ledger.load()['authentication']['accountMutated'])
            order.append('auth')
        c.tun.cleanup.side_effect = lambda *a, **kw: order.append('product') or {'state': 'forced'}
        with patch.object(m, 'close_auth', side_effect=auth), patch.object(m, 'close_transactions'):
            result = c.abort()
        self.assertEqual(order, ['lease', 'auth', 'product'])
        self.assertEqual(result['state'], 'failed')
        self.assertTrue(result['forced']); self.assertTrue(c.failed)
        self.assertEqual(c.ledger.load()['result'], 'failed')

    def test_relock_failure_does_not_claim_recovery_or_cancel_watchdog(self):
        c = controller(m.AUTH_MODE)
        c.ledger.value['authentication']['accountMutated'] = True
        c.lease = Mock(); c.lease.close.side_effect = ValueError('relock unproved')
        c.tun.cleanup.return_value = {'state': 'failed'}
        with patch.object(m, 'close_transactions'):
            result = c.abort()
        c.lease.close.assert_called_once()
        self.assertEqual(result['state'], 'failed')
        self.assertIn('authentication-cleanup-unknown', result['unknown'])
        self.assertTrue(c.ledger.load()['authentication']['accountMutated'])
        self.assertTrue(c.ledger.load()['authenticationWatchdogActive'])
        c.ops.run.assert_not_called()
        c.tun.cleanup.assert_called_once_with(c.ledger, forced=True)


class LaunchTests(unittest.TestCase):
    def test_valid_closed_default(self): self.assertIs(m.validate_launch(v := launch()), v)
    def test_valid_permission_is_not_authentication(self):
        v = launch(m.AUTH_MODE); self.assertIs(m.validate_launch(v), v)
        self.assertNotIn('authenticated', v['authorization'])
    def test_unknown_fields_denied(self):
        v = launch(); v['command'] = '/bin/sh'
        with self.assertRaises(ValueError): m.validate_launch(v)
    def test_bad_code_inventory(self):
        for field in ('omit', 'extra'):
            v = launch()
            if field == 'omit': del v['codeHashes']['fixtures.py']
            else: v['codeHashes']['evil.py'] = H
            with self.assertRaises(ValueError): m.validate_launch(v)
    def test_root_gui_denied(self):
        for uid in (0, -1, True, '1001'):
            v = launch(); v['guiUid'] = uid
            with self.assertRaises(ValueError): m.validate_launch(v)
    def test_runtime_outside_home_denied(self):
        for p in ('/etc/shadow', '/tmp/qa/home/../other', 'relative'):
            v = launch(); v['runtimePath'] = p
            with self.assertRaises(ValueError): m.validate_launch(v)
    def test_wrong_kernel_denied(self):
        v = launch(); v['kernelCommit'] = '814' + 'a' * 37
        with self.assertRaises(ValueError): m.validate_launch(v)
    def test_boolean_authority_denied(self):
        v = launch(m.AUTH_MODE); v['authorization'] = True
        with self.assertRaises(ValueError): m.validate_launch(v)
    def test_permission_absent_denied(self):
        v = launch(m.AUTH_MODE); v['authorization'] = None
        with self.assertRaises(ValueError): m.validate_launch(v)
    def test_permission_exact_phrase(self):
        v = launch(m.AUTH_MODE); v['authorization']['phrase'] = 'allow TUN'
        with self.assertRaises(ValueError): m.validate_permission(v)
    def test_permission_source_run_actor_bindings(self):
        for field in m.EXECUTION_KEYS:
            v = launch(m.AUTH_MODE); v['authorization']['execution'][field] += 'x'
            with self.assertRaises(ValueError): m.validate_permission(v)
    def test_permission_uid_account_actions(self):
        for field, bad in (('guiUid', 0), ('runToken', 'f' * 12), ('account', 'root:evil'),
                           ('transactions', ['install']), ('maxLeaseSeconds', 999)):
            v = launch(m.AUTH_MODE); v['authorization'][field] = bad
            with self.assertRaises(ValueError): m.validate_permission(v)
    def test_permission_times(self):
        now = int(time.time())
        for begin, end in ((now + 10, now + 100), (now - 100, now - 1), (now, now + 10), (now, now + 901)):
            v = launch(m.AUTH_MODE); v['authorization'].update(notBeforeUnix=begin, expiresUnix=end)
            with self.assertRaises(ValueError): m.validate_permission(v)
    def test_recovery_does_not_expire_relock_authority(self):
        v = launch(m.AUTH_MODE); v['authorization'].update(notBeforeUnix=1, expiresUnix=500)
        with self.assertRaises(ValueError): m.validate_launch(v)
        self.assertIs(m.validate_launch(v, recovery=True), v)
    def test_bootstrap_constant_no_root_import_runner_paths(self):
        self.assertNotIn('import root_controller', m.BOOTSTRAP)
        self.assertIn('os.O_EXCL|os.O_NOFOLLOW', m.BOOTSTRAP)
        self.assertIn("os.execve('/usr/bin/python3'", m.BOOTSTRAP)
        self.assertIn("d/'root_controller.py'", m.BOOTSTRAP)
    def test_envelope_requires_exact_source_hash(self):
        with tempfile.TemporaryDirectory() as d:
            v = launch()
            for name in m.FILES: Path(d, name).write_text('test')
            with self.assertRaises(ValueError): m.bootstrap_envelope(v, d)
            v['codeHashes'] = dict.fromkeys(m.FILES, m.sha(b'test'))
            e = json.loads(m.bootstrap_envelope(v, d)); self.assertEqual(set(e['files']), set(m.FILES))


class ProtocolTests(unittest.TestCase):
    def test_fixed_action_no_exec_kill_path(self):
        for action in ('exec', 'kill', 'password', 'register_pid', 'read_file', 'apply', 'plan'):
            with self.assertRaises(ValueError): controller().dispatch({'seq': 1, 'action': action})
    def test_sequence_and_bool_rejected(self):
        for seq in (0, 2, True, '1'):
            with self.assertRaises(ValueError): controller().dispatch({'seq': seq, 'action': 'core_attest'})
    def test_extra_path_denied(self):
        with self.assertRaises(ValueError): controller().dispatch({'seq': 1, 'action': 'capture_read', 'offset': 0, 'path': '/etc/shadow'})
    def test_replay_denied(self):
        c = controller(); c.core_attest = Mock(return_value={})
        c.dispatch({'seq': 1, 'action': 'core_attest'})
        with self.assertRaises(ValueError): c.dispatch({'seq': 1, 'action': 'core_attest'})
    def test_failed_cannot_resume(self):
        c = controller(); c.failed = True
        with self.assertRaises(ValueError): c.dispatch({'seq': 1, 'action': 'core_attest'})
    def test_rpc_exact_fields(self):
        c = controller(); c.rpc = Mock(return_value={})
        c.dispatch({'seq': 1, 'action': 'rpc', 'method': 'status', 'transactionId': 't', 'runtimeIncarnation': 'i'})
        c.rpc.assert_called_once_with('status', 't', 'i')
        with self.assertRaises(ValueError): c.dispatch({'seq': 2, 'action': 'rpc', 'method': 'capabilities', 'secret': 'x'})
    def test_reserved_recovery_margin(self):
        c = controller(); c.ledger.value['deadlineMonotonic'] = time.monotonic() + 29
        with self.assertRaises(ValueError): c.guard()
    def test_missing_auth_watchdog(self):
        c = controller(); c.ledger.value['authenticationWatchdogActive'] = False
        with self.assertRaises(ValueError): c.guard()
    def test_existing_readiness_does_not_assume_agent(self):
        c = controller()
        with patch.object(m.subprocess, 'run', return_value=types.SimpleNamespace(returncode=1)) as run:
            with self.assertRaises(ValueError): c.authentication_readiness()
            self.assertNotIn('--allow-user-interaction', run.call_args.args[0])
        with patch.object(m.subprocess, 'run', return_value=types.SimpleNamespace(returncode=0)):
            self.assertFalse(c.authentication_readiness()['authenticated'])
    def test_no_network_before_actual_install_auth(self):
        c = controller(); c.ledger.value['authentication']['authorized'] = False
        with self.assertRaises(ValueError): c.network_start()
    def test_register_helper_not_from_bool_only(self):
        c = controller(); c.ledger.value['authentication']['authorized'] = True
        with self.assertRaises((ValueError, IndexError)): c.dispatch({'seq': 1, 'action': 'register_helper'})
    def test_root_core_registration_does_not_take_pid(self):
        with self.assertRaises(ValueError): controller().dispatch({'seq': 1, 'action': 'register_core', 'pid': 1})
    def test_core_stopped_reuse_rejected(self):
        c = controller(); c.ledger.value['processes'] = [{'role': 'core', 'identity': identity()}]
        c.ops.proc.return_value = {**identity(), 'startTime': 'changed'}
        with self.assertRaises(ValueError): c.core_stopped()
        c.ops.proc.return_value = None; self.assertTrue(c.core_stopped()['stopped'])
    def test_capture_not_finished_or_replayed_denied(self):
        c = controller()
        with self.assertRaises(ValueError): c.capture_read(0)
        c.ledger.value['capture'] = {'sha256': H}; c.capture_offset = 10
        with self.assertRaises(ValueError): c.capture_read(0)
    def test_rpc_other_methods_denied_before_process_access(self):
        for action in ('apply', 'retry', 'ack', 'plan', 'suite'):
            with self.assertRaises(ValueError): controller().rpc(action)
    def test_transaction_begun_cannot_run_network(self):
        c = controller(); c.observer = Mock()
        with self.assertRaises(ValueError): c.dispatch({'seq': 1, 'action': 'network_start'})


class TimerTests(unittest.TestCase):
    def test_machine_duration_parser(self):
        self.assertEqual(m.duration_seconds('1min 30s'), 90)
        self.assertAlmostEqual(m.duration_seconds('1h 3ms'), 3600.003)
        for value in ('infinity', '90', '90s evil', 'NaN s'):
            with self.assertRaises(ValueError): m.duration_seconds(value)
    def timer(self, **change):
        unit = 'polaris-tun-lease-0123456789ab-install'; argv = ['/usr/bin/env', '-i', '/usr/bin/python3', '-I', '/run/frozen/controller.py']
        service = {'ExecStart': '{ path=/usr/bin/env ; argv[]=' + ' '.join(argv) + ' ; }', 'User': '', 'Group': '',
                   'DynamicUser': 'no', 'FragmentPath': '/run/systemd/transient/' + unit + '.service', 'DropInPaths': '', 'ActiveState': 'inactive'}
        timer = {'ActiveState': 'active', 'Unit': unit + '.service', 'TimersMonotonic': '[ { OnActiveUSec=1min 30s ; next_elapse=3min 10s } ]'}
        service.update(change.get('service', {})); timer.update(change.get('timer', {}))
        ops = Mock(); ops.run.side_effect = ['\n'.join(k+'='+v for k,v in service.items()), '\n'.join(k+'='+v for k,v in timer.items())]
        with patch.object(m.time, 'monotonic', return_value=100):
            return m.verify_timer(ops, types.SimpleNamespace(SYSTEMCTL='/usr/bin/systemctl'), unit, argv, 90, 100)
    def test_actual_timer_shape(self): self.assertTrue(self.timer()['rootFixedCommandVerified'])
    def test_wrong_service_command_user_dropin_failed(self):
        for field, value in (('ExecStart', '/bin/sh'), ('User', 'runner'), ('DynamicUser', 'yes'),
                             ('DropInPaths', '/etc/override'), ('ActiveState', 'active'), ('FragmentPath', '/tmp/unit')):
            with self.assertRaises(ValueError): self.timer(service={field: value})
    def test_timer_skip_cancel_wrong_deadline_failed(self):
        for field, value in (('ActiveState', 'inactive'), ('Unit', 'other.service'),
                             ('TimersMonotonic', '[ { OnActiveUSec=90s ; next_elapse=999s } ]')):
            with self.assertRaises(ValueError): self.timer(timer={field: value})


class ReadinessObservationTests(unittest.TestCase):
    def exercise(self, fault=None, status='L'):
        from contextlib import ExitStack
        c = controller(m.AUTH_MODE); c.ledger = None
        user = types.SimpleNamespace(pw_uid=1001, pw_name='runner', pw_shell='/bin/bash', pw_dir='/home/runner', pw_gid=1001, pw_gecos='PRIVATE')
        group = types.SimpleNamespace(gr_name='sudo', gr_mem=['runner'], gr_gid=2)
        if fault == 'permission-schema': c.launch['authorization']['extra'] = 'PRIVATE'
        if fault == 'permission-mode': c.launch['authorization']['phrase'] = 'PRIVATE'
        if fault == 'permission-binding': c.launch['authorization']['guiUid'] = 1002
        if fault == 'permission-actions': c.launch['authorization']['maxLeaseSeconds'] = 91
        if fault == 'permission-window': c.launch['authorization']['expiresUnix'] += 1000
        if fault == 'permission-time': c.launch['authorization']['notBeforeUnix'] -= 2000; c.launch['authorization']['expiresUnix'] -= 2000
        if fault == 'account-login': user.pw_shell = '/sbin/nologin'
        if fault == 'account-groups-required': group.gr_name = 'users'
        if fault == 'account-groups-records': group.gr_mem = None
        def tool(path):
            if fault == 'tool-' + Path(path).name: raise ValueError('PRIVATE')
            return {'dev': 1, 'inode': 2, 'sha256': H}
        c.tun.file_identity.side_effect = tool
        output = b'PRIVATE malformed' if fault == 'status-format' else ('runner ' + status + ' 2026-01-01 0 99999 7 -1\n').encode()
        with ExitStack() as stack:
            stack.enter_context(patch.object(m.pwd, 'getpwall', return_value=[user,user] if fault == 'account-unique' else [user]))
            stack.enter_context(patch.object(m.pwd, 'getpwnam', return_value=user))
            stack.enter_context(patch('grp.getgrall', return_value=[group], side_effect=OSError('PRIVATE') if fault == 'account-groups-enumerate' else None))
            run = stack.enter_context(patch.object(m.subprocess, 'run', return_value=types.SimpleNamespace(stdout=output),
                side_effect=OSError('PRIVATE') if fault == 'status-command' else None))
            spawn = stack.enter_context(patch.object(m.subprocess, 'Popen'))
            lease = stack.enter_context(patch.object(m.CredentialLease, 'open'))
            if fault or status == 'P':
                with self.assertRaises(Exception): c.prepare()
                c.tun.prepare_root.assert_not_called(); c.tun.collect_preflight.assert_not_called()
                self.assertEqual(c.stage, 'authentication-readiness')
            else:
                self.assertFalse(c.authentication_readiness()['authenticated'])
            spawn.assert_not_called(); lease.assert_not_called()
            for call in run.call_args_list:
                self.assertEqual(call.args[0], ['/usr/bin/passwd', '-S', 'runner'])
            receipt = c.abort(); m.validate_abort_receipt(receipt)
            self.assertFalse(receipt['prepared']); self.assertNotIn('productCleanup', receipt)
        self.assertNotIn('PRIVATE', json.dumps(receipt))
        return c.readonly_auth

    def test_every_readiness_guard_has_fixed_nonsecret_failure_observation(self):
        for fault in m.AUTH_GUARDS[:-2]:
            with self.subTest(fault=fault):
                value = self.exercise(fault)
                m.validate_auth_observation(value)
                self.assertEqual(value['guard'], fault); self.assertFalse(value['completed'])

    def test_p_remains_rejected_l_np_are_readiness_only(self):
        value = self.exercise(status='P')
        self.assertEqual(value['guard'], 'status-existing-password')
        self.assertEqual(value['passwordStatus'], 'P'); self.assertFalse(value['completed'])
        for status in ('L', 'NP'):
            value = self.exercise(status=status); self.assertEqual(value['passwordStatus'], status)
            self.assertTrue(value['completed'])

    def test_status_parser_errors_never_echo_raw_bytes(self):
        for raw in (b'PRIVATE', b'runner PRIVATE a b c d e', b'other L a b c d e', b'\xff', b'x'*4097):
            observation = m.auth_observation()
            with patch.object(m.subprocess, 'run', return_value=types.SimpleNamespace(stdout=raw)):
                with self.assertRaises(Exception): m.account_status('runner', observation=observation)
            self.assertEqual(observation['guard'], 'status-format')
            self.assertEqual(observation['passwordStatus'], 'unknown')
            self.assertNotIn('PRIVATE', json.dumps(observation))

    def test_status_timeout_and_command_failure_stay_unknown(self):
        for error in (m.subprocess.TimeoutExpired('fixed-passwd-status', 5), m.subprocess.CalledProcessError(1, 'fixed-passwd-status'), OSError('PRIVATE')):
            observation = m.auth_observation()
            with patch.object(m.subprocess, 'run', side_effect=error):
                with self.assertRaises(Exception): m.account_status('runner', observation=observation)
            self.assertEqual(observation['guard'], 'status-command')
            self.assertEqual(observation['passwordStatus'], 'unknown')
            self.assertNotIn('PRIVATE', json.dumps(observation))

    def test_prepared_abort_keeps_product_unknown_and_no_normal_claim(self):
        c = controller(); product = {'state': 'forced', 'forced': True,
            'unknown': ['earlier-failure-is-permanent', 'namespace-removal-unknown:pt01234567n'], 'publicationEligible': False}
        c.tun.cleanup.return_value = product
        with patch.object(m, 'close_auth'), patch.object(m, 'close_transactions'):
            receipt = c.abort()
        self.assertIs(m.validate_abort_receipt(receipt), receipt)
        self.assertTrue(receipt['prepared']); self.assertEqual(receipt['productCleanup'], product)
        self.assertEqual(receipt['state'], 'failed')
        c = controller(); c.tun.cleanup.side_effect = ValueError('PRIVATE')
        with patch.object(m, 'close_auth'), patch.object(m, 'close_transactions'):
            receipt = c.abort()
        m.validate_abort_receipt(receipt)
        self.assertEqual(receipt['productCleanup'], {'state': 'failed'})
        self.assertIn('product-cleanup-unknown', receipt['unknown'])
        self.assertNotIn('PRIVATE', json.dumps(receipt))

    def test_abort_receipt_validation_preserves_failure_and_rejects_injected_fields(self):
        c = controller(); c.ledger = None; c.preparation_pending = True
        value = c.abort(); self.assertIs(m.validate_abort_receipt(value), value)
        self.assertIsNone(value['prepared']); self.assertIn('root-preparation-incomplete', value['unknown'])
        for change in ({'state': 'normal'}, {'unknown': []}, {'extra': 'PRIVATE'}, {'actualReadOnlyAuth': {'raw': 'PRIVATE'}}, {'prepared': 0}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                m.validate_abort_receipt({**value, **change})
        c.preparation_pending = False; value = c.abort()
        self.assertEqual(value['unknown'], []); self.assertEqual(value['state'], 'failed')
        prepared = {**value, 'prepared': True, 'productCleanup': {'state': 'forced', 'forced': True, 'unknown': ['earlier-failure-is-permanent'], 'publicationEligible': False}}
        self.assertIs(m.validate_abort_receipt(prepared), prepared)
        for bad in ({'state': 'normal', 'forced': False, 'unknown': [], 'publicationEligible': False}, {'state': 'failed'}, {'state': 'forced', 'forced': True, 'unknown': ['PRIVATE'], 'publicationEligible': False}):
            with self.assertRaises(ValueError): m.validate_abort_receipt({**prepared, 'productCleanup': bad})


class CredentialTests(unittest.TestCase):
    def account(self, status='L', duplicate=False, admin=True, recovery=False):
        user = types.SimpleNamespace(pw_uid=1001, pw_name='runner', pw_shell='/bin/bash', pw_dir='/home/runner', pw_gid=1001, pw_gecos='')
        group = types.SimpleNamespace(gr_name='sudo' if admin else 'users', gr_mem=['runner'], gr_gid=2)
        with patch.object(m.pwd, 'getpwall', return_value=[user, user] if duplicate else [user]), \
             patch.object(m.pwd, 'getpwnam', return_value=user), patch('grp.getgrall', return_value=[group]), \
             patch.object(m, 'account_status', return_value=status):
            return m.credential_account(launch(m.AUTH_MODE), Mock(), recovery=recovery)
    def test_original_locked_or_no_password_only(self):
        self.assertEqual(self.account('L')[1], 'L'); self.assertEqual(self.account('NP')[1], 'NP')
        with self.assertRaises(ValueError): self.account('P')
    def test_no_new_account_or_admin_membership(self):
        with self.assertRaises(ValueError): self.account(duplicate=True)
        with self.assertRaises(ValueError): self.account(admin=False)
    def test_recovery_still_locks_if_membership_changed(self): self.assertEqual(self.account('P', admin=False, recovery=True)[1], 'P')
    def test_relock_fixed_existing_account_stdin_no_secret(self):
        user = types.SimpleNamespace(pw_name='runner', pw_uid=1001)
        with patch.object(m, 'credential_account', return_value=(user, 'P')), patch.object(m, 'account_status', return_value='L'), \
             patch.object(m.subprocess, 'run') as run:
            self.assertEqual(m.relock_account(launch(m.AUTH_MODE), Mock())['finalPasswordStatus'], 'L')
            self.assertEqual(run.call_args.args[0], ['/usr/bin/passwd', '-l', 'runner'])
            self.assertNotIn('input', run.call_args.kwargs)
    def test_relock_requires_actual_l(self):
        with patch.object(m, 'credential_account', return_value=(types.SimpleNamespace(pw_name='runner', pw_uid=1001), 'P')), \
             patch.object(m, 'account_status', return_value='P'), patch.object(m.subprocess, 'run'):
            with self.assertRaises(ValueError): m.relock_account(launch(m.AUTH_MODE), Mock())
    def test_no_setting_before_real_agent_registration(self):
        c = controller(m.AUTH_MODE); lease = m.CredentialLease(c, 'install')
        with patch.object(m, 'credential_account', return_value=(types.SimpleNamespace(pw_name='runner', pw_uid=1001), 'L')), \
             patch.object(m.subprocess, 'run') as run:
            with self.assertRaises(ValueError): lease.open()
            run.assert_not_called()
    def test_setting_unknown_always_attempts_relock(self):
        c = controller(m.AUTH_MODE); lease = m.CredentialLease(c, 'install')
        c.agent = Mock(); c.agent.poll.return_value = None; c.ledger.value['authentication']['registered'] = True
        c.ops.run.return_value = 'inactive'
        writer = Mock(pid=89); writer.communicate.side_effect = subprocess.TimeoutExpired('chpasswd', 10); writer.poll.return_value = None
        c.ops.proc.return_value = {**identity(89, 0), 'ppid': os.getpid()}
        c.tun.executable_identity.return_value = c.tun.file_identity.return_value = {'sha256': H}
        with patch.object(m, 'credential_account', return_value=(types.SimpleNamespace(pw_name='runner', pw_uid=1001), 'L')), \
             patch.object(m, 'verify_timer'), patch.object(m, 'account_status', return_value='L'), \
             patch.object(m, 'setting_intent'), patch.object(m, 'independent_record'), \
             patch.object(m.subprocess, 'Popen', return_value=writer), patch.object(m.os, 'pidfd_open', return_value=99), \
             patch.object(m.os, 'close'), patch.object(m.signal, 'pidfd_send_signal') as stop, \
             patch.object(m, 'relock_account', return_value=credential_proof()) as relock:
            with self.assertRaises(subprocess.TimeoutExpired): lease.open()
            stop.assert_called_once_with(99, signal.SIGKILL); relock.assert_called_once()
            self.assertTrue(lease.closed); self.assertFalse(lease.secret)
    def test_timer_failure_no_chpasswd(self):
        c = controller(m.AUTH_MODE); lease = m.CredentialLease(c, 'install'); c.agent = Mock()
        c.ledger.value['authentication']['registered'] = True
        with patch.object(m, 'credential_account', return_value=(types.SimpleNamespace(pw_name='runner', pw_uid=1001), 'L')), \
             patch.object(m, 'verify_timer', side_effect=ValueError('timer')), patch.object(m.subprocess, 'run') as run:
            with self.assertRaises(ValueError): lease.open()
            run.assert_not_called(); self.assertFalse(lease.attempted)
    def test_lease_close_locks_before_broken_ledger(self):
        c = controller(m.AUTH_MODE); lease = m.CredentialLease(c, 'install'); lease.attempted = True; lease.secret = bytearray(b'private')
        c.ledger.mutate = Mock(side_effect=OSError('ledger unavailable'))
        with patch.object(m, 'relock_account', return_value=credential_proof()) as relock:
            with self.assertRaises(OSError): lease.close()
            relock.assert_called_once(); self.assertFalse(lease.secret)
    def test_auth_cleanup_never_kills_other_birth(self):
        c = controller(); c.ledger.value['authentication']['agent'] = {'identity': identity(90)}
        c.ops.proc.return_value = {**identity(90), 'startTime': 'reused'}
        with self.assertRaises(ValueError): m.close_auth(c.ledger, c.tun, False)
        c.ops.signal_owned.assert_not_called()
    def test_agent_sigkill_never_normal(self):
        c = controller(); old = identity(90); c.ledger.value['authentication']['agent'] = {'identity': old}
        c.ops.proc.return_value = old; c.ops.signal_owned.return_value = False
        with self.assertRaises(ValueError): m.close_auth(c.ledger, c.tun, False)
        self.assertEqual(c.ledger.value['result'], 'failed')
    def test_normal_close_no_signal_to_live_transaction(self):
        c = controller(); old = identity(90, 0); c.ledger.value['transactionProcesses'] = [{'identity': old}]
        c.ops.proc.return_value = old
        with self.assertRaises(ValueError): m.close_transactions(c.ledger, c.tun, signal_live=False)
        c.ops.signal_owned.assert_not_called()
    def test_observer_missing_pkexec_is_not_authorization(self):
        c = controller(); obs = m.TransactionObserver(c, 'install'); obs.thread = Mock(); obs.thread.is_alive.return_value = False
        obs.shells[(10, '1')] = {}; obs.rows = {}
        with self.assertRaises(ValueError): obs.finish()
    def test_observer_error_permanent(self):
        obs = m.TransactionObserver(controller(), 'install'); obs.error = 'missing'
        with self.assertRaises(ValueError): obs.snapshot()
    def test_watchdog_relock_precedes_mutable_ledger_load(self):
        c = controller(m.AUTH_MODE); calls = []
        c.tun.Ledger.side_effect = lambda _: calls.append('ledger') or (_ for _ in ()).throw(ValueError('bad ledger'))
        with tempfile.TemporaryDirectory() as d, patch.object(m, 'has_setting_intent', return_value=True), \
             patch.object(m, 'relock_account', side_effect=lambda *_: calls.append('relock') or {}):
            with self.assertRaises(ValueError): m.watchdog_recover(c.launch, c.tun, Path(d), 'install')
        self.assertEqual(calls[:2], ['relock', 'ledger'])


class AdditionalContracts(unittest.TestCase):
    def test_credential_rerun_attempt_closed_even_matching_objects(self):
        v = launch(m.AUTH_MODE); v['execution']['githubRunAttempt'] = '2'; v['authorization']['execution']['githubRunAttempt'] = '2'
        with self.assertRaises(ValueError): m.validate_permission(v)
    def test_core_scan_skips_pid_one(self):
        c = controller(); c.ledger.value['helper'] = {'identity': identity(70, 0)}
        child = identity(80); child['ppid'] = 70
        c.ops.proc.side_effect = lambda pid: child if pid == 80 else (_ for _ in ()).throw(AssertionError('PID1 must not be read'))
        with patch.object(m.Path, 'iterdir', return_value=[Path('/proc/1'), Path('/proc/self'), Path('/proc/80')]), patch.object(m,'proc_stat',return_value=child):
            c.register_core()
        c.tun.register_core.assert_called_once_with(c.ledger, 80)
    def test_transaction_scan_skips_pid_one(self):
        c = controller(); c.ops.proc.side_effect = lambda pid: c.gui if pid == c.gui['pid'] else (_ for _ in ()).throw(AssertionError('PID1'))
        obs = m.TransactionObserver(c, 'install')
        with patch.object(m.Path, 'iterdir', return_value=[Path('/proc/1'), Path('/proc/self')]): obs.scan()
        self.assertFalse(obs.snapshot()['authorizationObserved'])
    def test_private_read_rejects_symlink(self):
        with tempfile.TemporaryDirectory() as d:
            home = Path(d); home.chmod(0o700); target = home/'target'; target.write_text('x'); target.chmod(0o600)
            alias = home/'alias'; alias.symlink_to(target)
            with self.assertRaises(OSError): m.private_bytes(alias, os.getuid(), home)
    def test_private_read_has_actual_bytes_hash_no_public_original(self):
        with tempfile.TemporaryDirectory() as d:
            home = Path(d); home.chmod(0o700); target = home/'runtime'; target.write_bytes(b'{"secret":"fixture"}'); target.chmod(0o600)
            data, proof = m.private_bytes(target, os.getuid(), home)
            self.assertEqual(proof['sha256'], m.sha(data)); self.assertNotIn('secret', json.dumps(proof))
    def test_private_read_rejects_public_file(self):
        with tempfile.TemporaryDirectory() as d:
            home = Path(d); home.chmod(0o700); target = home/'runtime'; target.write_text('fixture'); target.chmod(0o644)
            with self.assertRaises(ValueError): m.private_bytes(target, os.getuid(), home)
    def pump(self, chunks, *, echo=0, name='runner'):
        c = controller(m.AUTH_MODE); c.pty = 90; c.auth_reached('transaction-guard', 'install')
        lease = m.CredentialLease(c, 'install'); lease.window = 90
        lease.user = types.SimpleNamespace(pw_name=name, pw_gecos=''); lease.secret = bytearray(b'never-log-this-password')
        lease.close = Mock(); writes = []
        with patch.object(m.select, 'select', return_value=([90], [], [])), patch.object(m.os, 'read', side_effect=chunks), \
             patch.object(m.os, 'write', side_effect=lambda fd,b: writes.append(b) or len(b)), \
             patch('termios.tcgetattr', return_value=[0,0,0,echo,0,0,[]]):
            lease.pump()
        return lease, writes
    def test_real_prompt_account_and_complete_separate(self):
        lease, writes = self.pump([b'==== AUTHENTICATING FOR org.freedesktop.policykit.exec ====\nAuthenticating as: runner\nPassword: ',
                                  b'==== AUTHENTICATION COMPLETE ====\n'])
        self.assertTrue(lease.authenticated); self.assertEqual(writes, [b'never-log-this-password\n']); self.assertIsNone(lease.error)
    def test_wrong_prompt_identity_never_gets_secret(self):
        lease, writes = self.pump([b'==== AUTHENTICATING FOR org.freedesktop.policykit.exec ====\nAuthenticating as: root\nPassword: '])
        self.assertFalse(lease.authenticated); self.assertEqual(writes, []); lease.close.assert_called_once()
    def test_unrelated_action_never_gets_secret(self):
        lease, writes = self.pump([b'==== AUTHENTICATING FOR other.action ====\nAuthenticating as: runner\nPassword: '])
        self.assertEqual(writes, []); self.assertIsNotNone(lease.error)
    def test_completion_without_password_is_not_authentication(self):
        lease, writes = self.pump([b'==== AUTHENTICATION COMPLETE ====\n'])
        self.assertFalse(lease.authenticated); self.assertEqual(writes, [])
    def test_multiple_identity_selects_only_exact_existing_account(self):
        lease, writes = self.pump([b'==== AUTHENTICATING FOR org.freedesktop.policykit.exec ====\nMultiple identities can be used for authentication:\n 1.  root\n 2.  runner\nChoose identity to authenticate as (1-2): ',
                                  b'Password: ', b'==== AUTHENTICATION COMPLETE ====\n'])
        self.assertTrue(lease.authenticated); self.assertEqual(writes, [b'2\n', b'never-log-this-password\n'])
    def test_multiple_identity_ambiguity_denied(self):
        lease, writes = self.pump([b'==== AUTHENTICATING FOR org.freedesktop.policykit.exec ====\n 1.  runner\n 2.  runner\nChoose identity to authenticate as (1-2): '])
        self.assertEqual(writes, []); self.assertIsNotNone(lease.error)
    def test_authentication_denial_closes_lease(self):
        lease, writes = self.pump([b'==== AUTHENTICATION FAILED ====\n'])
        self.assertEqual(writes, []); lease.close.assert_called_once()
    def test_missing_notify_does_not_become_auth(self):
        c = controller(m.AUTH_MODE); c.ledger.value['authentication'].update(registered=True, authorized=False)
        self.assertFalse(c.ledger.load()['authentication']['authorized'])


class RecoveryAndObserverTests(unittest.TestCase):
    def observer_case(self, include_pkexec=True):
        c = controller(); c.tools = {'pkexec': {'sha256': H}, 'bash': {'sha256': H}}
        c.verify_script = Mock(return_value={'sha256': H}); c.tun.executable_identity.return_value = {'sha256': H}
        obs = m.TransactionObserver(c, 'install')
        process = {**identity(60), 'ppid': c.gui['pid'], 'exe': str(Path('/usr/bin/pkexec').resolve())}
        script = c.launch['helperScriptDir'] + '/' + 'd'*24 + '-polaris-helper-install.sh'
        rows = {c.gui['pid']: c.gui, 60: process}; c.ops.proc.side_effect = lambda pid: rows.get(pid)
        c.tun.transaction_identity.side_effect = lambda pid: {'identity': rows[pid], 'uids': (1001,0,0,0) if rows[pid]['uid']==1001 else (0,0,0,0),
            'argv': ['/usr/bin/pkexec','/bin/bash',script] if rows[pid]['uid']==1001 else ['/bin/bash',script], 'executable': {'sha256': H}}
        with patch.object(m.Path, 'iterdir', side_effect=lambda: [Path('/proc/1'), *[Path('/proc', str(k)) for k in rows]]), \
             patch.object(m, 'command_line', return_value=['/usr/bin/pkexec','/bin/bash',script]), \
             patch.object(m, 'uid_tuple', return_value=(1001,0,0,0)):
            with patch.object(m, 'proc_stat', side_effect=lambda pid: rows.get(pid)):
                if include_pkexec: obs.scan()
        rows[60] = {**process, 'uid': 0, 'exeInode': 88, 'exe': str(Path('/bin/bash').resolve())}
        with patch.object(m.Path, 'iterdir', side_effect=lambda: [Path('/proc/1'), *[Path('/proc', str(k)) for k in rows]]), \
             patch.object(m, 'command_line', return_value=['/bin/bash',script]), \
             patch.object(m, 'uid_tuple', return_value=(0,0,0,0)):
            with patch.object(m, 'proc_stat', side_effect=lambda pid: rows.get(pid)): obs.scan()
        return c, obs, rows
    def test_actual_observer_pkexec_same_birth_root_exec(self):
        c, obs, rows = self.observer_case()
        self.assertTrue(obs.snapshot()['authorizationObserved']); self.assertEqual(obs.snapshot()['pkexecBirths'], obs.snapshot()['rootShellBirths'])
        self.assertEqual(obs.snapshot()['subject'], {'pid': c.gui['pid'], 'startTime': c.gui['startTime'], 'uid': c.gui['uid']})
    def test_unobserved_root_shell_is_fail_not_inferred(self):
        with self.assertRaises(ValueError): self.observer_case(include_pkexec=False)
    def test_reparented_root_not_reconstructed_from_argv(self):
        c = controller(); obs = m.TransactionObserver(c, 'install')
        orphan = {**identity(60,0), 'ppid': 1}; rows = {c.gui['pid']:c.gui,60:orphan}
        c.ops.proc.side_effect = lambda pid: rows.get(pid)
        with patch.object(m.Path, 'iterdir', return_value=[Path('/proc/1'),Path('/proc/60')]): obs.scan()
        self.assertEqual(obs.snapshot()['owned'], [])
    def test_auth_subtree_tracks_actual_child_and_skips_pid_one(self):
        c = controller(); agent = identity(90); child = {**identity(91,0), 'ppid':90}
        c.ledger.value['authentication']['agent'] = {'identity':agent}; rows={90:agent,91:child}
        c.ops.proc.side_effect=lambda pid: rows.get(pid)
        observer=m.AuthObserver(c,'install')
        with patch.object(m.Path,'iterdir',return_value=[Path('/proc/1'),Path('/proc/90'),Path('/proc/91')]), \
             patch.object(m,'write_recovery_snapshot') as write, patch.object(m,'proc_stat',side_effect=lambda pid:rows.get(pid)):
            observer.scan()
            saved=write.call_args.args[2]; self.assertEqual(len(saved['processes']),2)
            self.assertEqual(saved['processes'][1]['uid'],0)
    def test_fresh_usable_password_rejection_never_locks(self):
        c=controller(m.AUTH_MODE); lease=m.CredentialLease(c,'install'); c.agent=Mock()
        c.ledger.value['authentication']['registered']=True; c.ops.run.return_value='inactive'
        with patch.object(m,'credential_account',return_value=(types.SimpleNamespace(pw_name='runner',pw_uid=1001),'L')), \
             patch.object(m,'verify_timer'),patch.object(m,'account_status',return_value='P'), \
             patch.object(m,'setting_intent') as intent,patch.object(m,'relock_account') as relock, \
             patch.object(m.subprocess,'Popen') as writer:
            with self.assertRaises(ValueError): lease.open()
            intent.assert_not_called(); relock.assert_not_called(); writer.assert_not_called(); self.assertFalse(lease.attempted)
    def test_watchdog_without_setting_intent_does_not_touch_account(self):
        c=controller(m.AUTH_MODE); c.tun.Ledger.side_effect=ValueError('ledger')
        with tempfile.TemporaryDirectory() as d,patch.object(m,'has_setting_intent',return_value=False), \
             patch.object(m,'relock_account') as relock:
            with self.assertRaises(ValueError): m.watchdog_recover(c.launch,c.tun,Path(d),'install')
            relock.assert_not_called()
    def test_watchdog_stops_writer_and_controller_before_final_relock(self):
        c=controller(m.AUTH_MODE); calls=[]; c.tun.Ledger.side_effect=ValueError('ledger')
        with tempfile.TemporaryDirectory() as d:
            directory=Path(d); (directory/'writer-install.json').write_text(json.dumps(identity(80,0)))
            (directory/'controller-birth.json').write_text(json.dumps(identity(81,0)))
            with patch.object(m,'has_setting_intent',return_value=True),patch.object(m.Path,'iterdir',return_value=[]), \
                 patch.object(m,'stop_recorded',side_effect=lambda ops,tun,old:calls.append(('stop',old['pid']))), \
                 patch.object(m,'relock_account',side_effect=lambda *_:calls.append(('relock',1001)) or {}):
                with self.assertRaises(ValueError): m.watchdog_recover(c.launch,c.tun,directory,'install')
        self.assertEqual(calls,[('stop',81),('stop',80),('relock',1001)])
    def test_invalid_setting_intent_binding_denied(self):
        c=controller(m.AUTH_MODE)
        with tempfile.TemporaryDirectory() as d:
            directory=Path(d); m.setting_intent(directory,'install',c.launch,'L')
            self.assertTrue(m.has_setting_intent(directory,'install',c.launch,c.tun))
            v=json.loads((directory/'setting-intent-install.json').read_text());v['account']='other'
            (directory/'setting-intent-install.json').write_text(json.dumps(v))
            with self.assertRaises(ValueError):m.has_setting_intent(directory,'install',c.launch,c.tun)
    def test_writer_cannot_receive_secret_before_birth_record(self):
        c=controller(m.AUTH_MODE); lease=m.CredentialLease(c,'install');c.agent=Mock();c.ops.run.return_value='inactive'
        c.ledger.value['authentication']['registered']=True
        writer=Mock(pid=89,returncode=0);writer.poll.return_value=0
        c.ops.proc.return_value={**identity(89,0),'ppid':os.getpid()};c.tun.executable_identity.return_value=c.tun.file_identity.return_value={'sha256':H}
        calls=[];writer.communicate.side_effect=lambda **kw:calls.append('secret')
        fake_thread=Mock()
        with patch.object(m,'credential_account',return_value=(types.SimpleNamespace(pw_name='runner',pw_uid=1001),'L')), \
             patch.object(m,'verify_timer'),patch.object(m,'account_status',side_effect=['L','P']), \
             patch.object(m,'setting_intent',side_effect=lambda *_:calls.append('intent')), \
             patch.object(m,'independent_record',side_effect=lambda *_:calls.append('writer-birth')), \
             patch.object(m.subprocess,'Popen',return_value=writer) as spawn,patch.object(m.os,'pidfd_open',return_value=99), \
             patch.object(m.threading,'Thread',return_value=fake_thread):
            lease.open()
        self.assertEqual(calls,['intent','writer-birth','secret']);self.assertEqual(spawn.call_args.args[0],['/usr/sbin/chpasswd','--crypt-method','SHA512','--sha-rounds','100000'])
        lease.secret.clear(); lease.writer_fd=None


class EntryAndConcurrencyTests(unittest.TestCase):
    def test_echo_still_enabled_never_receives_secret(self):
        c=controller(m.AUTH_MODE);c.pty=90;lease=m.CredentialLease(c,'install');lease.window=90
        lease.user=types.SimpleNamespace(pw_name='runner',pw_gecos='');lease.secret=bytearray(b'private');lease.close=Mock()
        chunk=b'==== AUTHENTICATING FOR org.freedesktop.policykit.exec ====\nAuthenticating as: runner\nPassword: '
        import termios
        with patch.object(m.time,'monotonic',side_effect=[0,1,2,3,4]),patch.object(m.time,'sleep'), \
             patch.object(m.select,'select',return_value=([90],[],[])),patch.object(m.os,'read',return_value=chunk), \
             patch('termios.tcgetattr',return_value=[0,0,0,termios.ECHO,0,0,[]]),patch.object(m.os,'write') as write:
            lease.pump()
        write.assert_not_called();lease.close.assert_called_once();self.assertFalse(lease.authenticated)
    def test_close_concurrency_is_serial_and_no_duplicate_relock(self):
        c=controller(m.AUTH_MODE);c.auth_reached('lease-account','install');lease=m.CredentialLease(c,'install');lease.attempted=True
        c.ledger.value['authentication']['lease']={};c.ops.run.return_value='inactive'
        with patch.object(m,'relock_account',return_value=credential_proof()) as relock:
            lease.close();lease.close()
        relock.assert_called_once();self.assertTrue(lease.closed)
    def test_stuck_private_worker_still_relocks_before_failure(self):
        c=controller(m.AUTH_MODE);c.auth_reached('lease-account','install');lease=m.CredentialLease(c,'install');lease.attempted=True;lease.secret=bytearray(b'private')
        lease.thread=Mock();lease.thread.is_alive.return_value=True;c.ops.run.return_value='inactive'
        c.ledger.value['authentication']['lease']={}
        with patch.object(m,'relock_account',return_value=credential_proof()) as relock:
            with self.assertRaises(ValueError):lease.close()
        relock.assert_called_once();self.assertFalse(lease.secret);self.assertFalse(lease.closed);c.ops.run.assert_not_called()
    def main_fixture(self, dispatch_error=False, finish=False):
        c=Mock();c.finished=False;c.seq=1;c.auth_snapshot.return_value=m.auth_runtime_observation('existing-session')
        def dispatch(_):
            if dispatch_error:raise ValueError('PRIVATE SECRET MUST NOT BE PRINTED')
            if finish:c.finished=True
            return {'seq':1,'ok':True,'result':{}}
        c.readonly_auth = None; c.vm_preflight = None
        c.dispatch.side_effect=dispatch
        lines=[b'{"seq":1,"action":"prepare"}\n',ValueError('EOF')]
        out=io.StringIO()
        import ctypes
        with patch.object(m.sys,'argv',['controller','serve','--run-id','0123456789ab']), \
             patch.object(m,'load_frozen',return_value=(launch(),Mock(),Path('/root/frozen'))), \
             patch.object(m,'Controller',return_value=c),patch.object(ctypes,'CDLL',return_value=Mock(prctl=Mock(return_value=0))), \
             patch.object(m,'read_line',side_effect=lines),patch.object(m.sys,'stdout',out):
            if finish:m.main()
            else:
                with self.assertRaises(ValueError):m.main()
        return c,out.getvalue()
    def test_main_eof_forces_abort(self):
        c,out=self.main_fixture();c.abort.assert_called_once();self.assertIn('ready',out)
    def test_main_error_redacts_details_and_aborts(self):
        c,out=self.main_fixture(dispatch_error=True);c.fail.assert_called_once();c.abort.assert_called_once()
        self.assertNotIn('PRIVATE SECRET',out);self.assertIn('CONTROLLER_FAILED',out)
    def test_main_normal_finish_does_not_force_recovery(self):
        c,_=self.main_fixture(finish=True);c.abort.assert_not_called()
    def test_watchdog_entry_always_nonzero(self):
        with patch.object(m.sys,'argv',['controller','watch-lease','--run-id','0123456789ab','--transaction','install']), \
             patch.object(m,'load_frozen',return_value=(launch(m.AUTH_MODE),Mock(),Path('/root/frozen'))) as load, \
             patch.object(m,'watchdog_recover') as recover:
            with self.assertRaises(SystemExit) as result:m.main()
            self.assertEqual(result.exception.code,1);self.assertTrue(load.call_args.kwargs['recovery']);recover.assert_called_once()


class WatchdogRaceTests(unittest.TestCase):
    def race(self, writer_registered):
        c=controller(m.AUTH_MODE);events=[];c.tun.Ledger.side_effect=ValueError('mutable ledger unavailable')
        with tempfile.TemporaryDirectory() as d:
            directory=Path(d);(directory/'controller-birth.json').write_text(json.dumps(identity(81,0)))
            if writer_registered:(directory/'writer-install.json').write_text(json.dumps(identity(80,0)))
            def stopped(ops,tun,old):events.append(('stop',old['pid']))
            def intent(*_):
                self.assertEqual(events,[('stop',81)])
                events.append(('read-intent',True));return True
            with patch.object(m,'has_setting_intent',side_effect=intent),patch.object(m,'stop_recorded',side_effect=stopped), \
                 patch.object(m,'relock_account',side_effect=lambda *_:events.append(('relock',1001)) or {}):
                with self.assertRaises(ValueError):m.watchdog_recover(c.launch,c.tun,directory,'install')
        return events
    def test_timeout_after_intent_before_writer_registration_stops_producer_first(self):
        self.assertEqual(self.race(False),[('stop',81),('read-intent',True),('relock',1001)])
    def test_timeout_after_writer_registration_before_secret_stops_both_first(self):
        self.assertEqual(self.race(True),[('stop',81),('read-intent',True),('stop',80),('relock',1001)])
    def test_failing_agent_ledger_cannot_skip_native_reap(self):
        # Structural ordering supplements registration mocks without opening a real PTY.
        source=Path(m.__file__).read_text();block=source[source.index('    def agent_register('):source.index('    def transaction_begin(')]
        self.assertLess(block.index('self.agent.kill(); self.agent.wait'),block.index('self.fail()'))
        self.assertIn('finally:\n                self.fail()',block)


class GuiBindingProofTests(unittest.TestCase):
    def binding(self, mismatch=False, changed=False):
        c=controller();c.gui=None;gui=identity();driver={**identity(20),'startTime':'1234','exeInode':5}
        c.ops.proc.side_effect=lambda pid:copy.deepcopy(gui if pid==50 else driver)
        c.tun.executable_identity.return_value={'sha256':H,'dev':1,'inode':78 if mismatch else 77}
        if changed:c.tun.same_process.side_effect=[True,False]
        with patch.object(m.Path,'iterdir',return_value=[Path('/proc/1'),Path('/proc/50')]), patch.object(m,'proc_stat',return_value=gui), patch.object(m,'uid_tuple',return_value=(1001,)*4):return c.bind_gui(50)
    def test_binding_returns_observed_executable_same_inode(self):
        result=self.binding();self.assertEqual(result['executable'],{'sha256':H,'dev':1,'inode':77})
        self.assertEqual(result['identity']['exeInode'],result['executable']['inode'])
    def test_binding_wrong_inode_rejected(self):
        with self.assertRaises(ValueError):self.binding(mismatch=True)
    def test_binding_birth_changed_after_hash_rejected(self):
        with self.assertRaises(ValueError):self.binding(changed=True)


class ZombieClosureTests(unittest.TestCase):
    def test_stat_only_reads_zombie_birth_without_exe(self):
        fields=['Z',str(os.getpid())]+['0']*17+['2345']
        with patch.object(m.Path,'read_text',return_value='91 (PAM (helper)) '+' '.join(fields)):
            self.assertEqual(m.proc_stat(91),{'pid':91,'startTime':'2345','state':'Z','ppid':os.getpid()})
    def test_missing_stat_but_existing_pid_is_unknown(self):
        with patch.object(m.Path,'read_text',side_effect=FileNotFoundError),patch.object(m,'proc_absent',return_value=False):
            with self.assertRaises(ValueError):m.proc_stat(91)
    def test_absence_requires_missing_proc_not_unreadable(self):
        with patch.object(m.Path,'lstat',side_effect=PermissionError):
            with self.assertRaises(PermissionError):m.proc_absent(91)
        with patch.object(m.Path,'lstat',side_effect=FileNotFoundError):self.assertTrue(m.proc_absent(91))
    def auth(self):
        c=controller(m.AUTH_MODE);c.agent=Mock(pid=90)
        c.ledger.value['authentication']['agent']={'identity':identity(90)}
        obs=m.AuthObserver(c,'install');obs.thread=Mock();obs.thread.is_alive.return_value=False;obs.scan=Mock()
        obs.rows[m.birth(identity(91,0))]=identity(91,0)
        return c,obs
    def test_known_adopted_zombie_uses_actual_waitpid(self):
        c,obs=self.auth()
        rows={90:{**identity(90),'state':'Z','ppid':os.getpid()},91:{**identity(91,0),'state':'Z','ppid':os.getpid()}}
        with patch.object(m.Path,'iterdir',return_value=[Path('/proc/90'),Path('/proc/91')]), \
             patch.object(m,'stop_recorded',return_value=False),patch.object(m,'proc_stat',side_effect=rows.get), \
             patch.object(m.os,'waitpid',return_value=(91,0)) as wait:
            obs.close()
        wait.assert_called_once_with(91,os.WNOHANG)
    def test_unknown_adopted_zombie_cannot_be_ignored(self):
        c,obs=self.auth()
        with patch.object(m.Path,'iterdir',return_value=[Path('/proc/92')]),patch.object(m,'stop_recorded',return_value=False), \
             patch.object(m,'proc_stat',return_value={**identity(92,0),'state':'Z','ppid':os.getpid()}):
            with self.assertRaises(ValueError):obs.close()
    def test_known_zombie_reuse_cannot_be_reaped(self):
        c,obs=self.auth()
        with patch.object(m.Path,'iterdir',return_value=[Path('/proc/91')]),patch.object(m,'stop_recorded',return_value=False), \
             patch.object(m,'proc_stat',return_value={**identity(91,0),'state':'Z','startTime':'999','ppid':os.getpid()}), \
             patch.object(m.os,'waitpid') as wait:
            with self.assertRaises(ValueError):obs.close()
        wait.assert_not_called()
    def test_waitpid_zero_not_reaped(self):
        c,obs=self.auth()
        with patch.object(m.Path,'iterdir',return_value=[Path('/proc/91')]),patch.object(m,'stop_recorded',return_value=False), \
             patch.object(m,'proc_stat',return_value={**identity(91,0),'state':'Z','ppid':os.getpid()}), \
             patch.object(m.os,'waitpid',return_value=(0,0)):
            with self.assertRaises(ValueError):obs.close()
    def test_full_identity_none_does_not_prove_secret_process_gone(self):
        c=controller();c.ops.proc.return_value=None
        with patch.object(m,'proc_stat',return_value={**identity(91,0),'state':'S'}):
            with self.assertRaises(ValueError):m.stop_recorded(c.ops,c.tun,identity(91,0))
        with patch.object(m,'proc_stat',return_value={**identity(91,0),'state':'Z'}):
            self.assertFalse(m.stop_recorded(c.ops,c.tun,identity(91,0)))
    def test_normal_cleanup_rejects_existing_pid_despite_full_identity_none(self):
        c=controller();c.ledger.value['authentication']['closed']=True
        c.ledger.value['processes']=[{'identity':identity(91,0)}];c.ops.proc.return_value=None
        with patch.object(m,'close_transactions'),patch.object(m,'proc_absent',return_value=False):
            with self.assertRaises(ValueError):c.normal_cleanup()
    def test_stop_agent_final_absence_is_independent_of_full_identity(self):
        c=controller();c.auth_observer=Mock();c.auth_observer.close.return_value={'ownedBirths':[[91,'2345']]}
        c.agent=Mock();c.agent.wait.return_value=-signal.SIGTERM;c.ops.proc.return_value=None
        with patch.object(m,'close_auth',return_value={}),patch.object(m,'proc_absent',return_value=False):
            with self.assertRaises(ValueError):c.stop_agent()


class StartupLifecycleTests(unittest.TestCase):
    def bootstrap_case(self, fault):
        # Ordinary temporary files plus mocked UID/isolated/root path/syscalls.
        # The fixed root bootstrap is never launched as a root process.
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp); stage_base = root / 'controller'; probe = root / 'probe'
            probe.write_bytes(b'mock-rpc')
            value = launch(); value['rpcProbePath'] = str(probe); value['rpcProbeSha256'] = m.sha(probe.read_bytes())
            files = {name: b'mock-frozen-source' for name in m.FILES}
            value['codeHashes'] = {name: m.sha(data) for name, data in files.items()}
            envelope = {'launch': value, 'files': {name: base64.b64encode(data).decode() for name, data in files.items()}}
            if fault == 'source-hash': envelope['files']['tun.py'] = base64.b64encode(b'changed').decode()
            output = io.StringIO()
            fake_sys = types.SimpleNamespace(flags=types.SimpleNamespace(isolated=True),
                stdin=types.SimpleNamespace(buffer=io.BytesIO((json.dumps(envelope) + '\n').encode())), stdout=output)
            def facts(info, uid=0, mode=None, nlink=None):
                return types.SimpleNamespace(st_mode=info.st_mode if mode is None else mode, st_uid=uid,
                    st_nlink=info.st_nlink if nlink is None else nlink, st_dev=info.st_dev,
                    st_ino=info.st_ino, st_size=info.st_size)
            class FakePath:
                def __init__(self, path): self.path = stage_base if str(path) == '/run/polaris-tun-controller' else Path(path)
                def __truediv__(self, other): return FakePath(self.path / other)
                def __fspath__(self): return str(self.path)
                def __str__(self): return str(self.path)
                @property
                def parents(self): return [FakePath(p) for p in self.path.parents]
                def exists(self): return self.path.exists()
                def mkdir(self, **kwargs): return self.path.mkdir(**kwargs)
                def rmdir(self): return self.path.rmdir()
                def lstat(self):
                    info = self.path.lstat()
                    return facts(info, mode=info.st_mode & ~0o077)
                def stat(self): return self.lstat()
            fake_os = types.SimpleNamespace(**{name: getattr(os, name) for name in dir(os) if not name.startswith('__')})
            fake_os.geteuid = lambda: 0
            def opened(path, flags, *args, **kwargs):
                if fault == 'directory-open' and flags & os.O_DIRECTORY: raise PermissionError('PRIVATE')
                return os.open(path, flags, *args, **kwargs)
            fake_os.open = opened
            def fstat(fd):
                info = os.fstat(fd); path = os.readlink('/proc/self/fd/' + str(fd))
                if path == str(probe):
                    return facts(info, uid=1001 if fault != 'probe-owner' else 0,
                                 nlink=2 if fault == 'probe-links' else info.st_nlink)
                return facts(info)
            fake_os.fstat = fstat
            fake_os.stat = lambda *a, **kw: facts(os.stat(*a, **kw))
            def execve(*args):
                directory = stage_base / value['runId']
                if fault == 'unknown-file': (directory / 'untouched').write_text('foreign')
                if fault == 'replaced-file':
                    (directory / 'tun.py').rename(directory / 'old-tun')
                    (directory / 'tun.py').write_text('replacement')
                raise OSError('PRIVATE')  # Never execute the frozen file or any command.
            fake_os.execve = Mock(side_effect=execve)
            if fault == 'unlink-failure': fake_os.unlink = Mock(side_effect=PermissionError('PRIVATE'))
            def importer(name, *args, **kwargs):
                return {'os': fake_os, 'sys': fake_sys, 'pathlib': types.SimpleNamespace(Path=FakePath)}.get(name) or builtins.__import__(name, *args, **kwargs)
            fake_builtins = dict(vars(builtins)); fake_builtins['__import__'] = importer
            fake_builtins['print'] = lambda *args, **kw: builtins.print(*args, **{**kw, 'file': output})
            with self.assertRaises(SystemExit) as caught:
                exec(compile(m.BOOTSTRAP, '<fixed-bootstrap-mocked>', 'exec'), {'__builtins__': fake_builtins})
            response = json.loads(output.getvalue())
            self.assertNotIn('PRIVATE', output.getvalue())
            self.assertFalse(response['ready'])
            directory = stage_base / value['runId']
            if fault == 'source-hash':
                self.assertFalse(stage_base.exists()); self.assertEqual(response['startupFailure']['stage'], 'sources')
            elif fault in ('directory-open', 'unknown-file', 'replaced-file', 'unlink-failure'):
                self.assertTrue(directory.exists())
                self.assertEqual(response['startupFailure']['stagingCleanup'], 'unconfirmed')
                self.assertEqual(caught.exception.code, 71)
            else:
                self.assertFalse(directory.exists()); self.assertTrue(stage_base.exists())
                self.assertEqual(response['startupFailure']['stagingCleanup'], 'removed')
                self.assertEqual(caught.exception.code, 70)
            if fault in ('probe-owner', 'probe-links'):
                fake_os.execve.assert_not_called(); self.assertEqual(response['startupFailure']['stage'], 'probe')
            if fault == 'replaced-file': self.assertEqual((directory / 'tun.py').read_text(), 'replacement')
            if fault == 'unknown-file': self.assertEqual((directory / 'untouched').read_text(), 'foreign')

    def test_fixed_bootstrap_failure_cleanup_owns_only_created_inodes(self):
        for fault in ('source-hash', 'directory-open', 'probe-owner', 'probe-links', 'exec',
                      'unknown-file', 'replaced-file', 'unlink-failure'):
            with self.subTest(fault=fault): self.bootstrap_case(fault)

    def test_post_exec_startup_failure_frames_retain_staging(self):
        import ctypes
        for fault in ('frozen-load', 'subreaper', 'controller-init'):
            with self.subTest(fault=fault):
                output = io.StringIO(); libc = Mock(); libc.prctl.return_value = 1 if fault == 'subreaper' else 0
                with patch.object(m.sys, 'argv', ['root_controller.py', 'serve', '--run-id', '0123456789ab']), \
                     patch.object(m, 'load_frozen', return_value=(launch(), Mock(), Path('/fixed')),
                                  side_effect=ValueError('PRIVATE') if fault == 'frozen-load' else None), \
                     patch.object(ctypes, 'CDLL', return_value=libc), \
                     patch.object(m, 'Controller', side_effect=ValueError('PRIVATE')) as ctor, \
                     patch.object(m.sys, 'stdout', output), self.assertRaises(SystemExit) as caught:
                    m.main()
                row = json.loads(output.getvalue())
                self.assertEqual(caught.exception.code, 72)
                self.assertEqual(row['startupFailure'], {'stage': fault, 'code': 'FROZEN_STARTUP_FAILED', 'stagingCleanup': 'retained'})
                self.assertNotIn('PRIVATE', output.getvalue())
                if fault != 'controller-init': ctor.assert_not_called()

    def test_bind_rejection_does_not_prepare_or_mutate_ledger(self):
        c = controller(); c.gui = None; c.ledger = None
        c.ops.proc.return_value = None
        with self.assertRaises(ValueError): c.bind_gui(50)
        self.assertEqual(c.stage, 'bind-gui')
        self.assertIsNone(c.ledger)
        c.tun.prepare_root.assert_not_called()
        receipt = c.abort(); m.validate_abort_receipt(receipt)
        self.assertFalse(receipt['prepared']); self.assertEqual(receipt['unknown'], [])
        self.assertIsNone(receipt['actualReadOnlyAuth'])

    def test_prepare_failure_matrix_preserves_original_guards_and_partial_ledger_unknown(self):
        for fault in ('authentication-readiness', 'vm-preflight', 'tool-identities', 'root-ledger',
                      'controller-ledger', 'auth-watchdog-start', 'auth-watchdog-verify'):
            with self.subTest(fault=fault):
                c = controller(); ledger = c.ledger; c.ledger = None
                c.authentication_readiness = Mock(return_value={}, side_effect=ValueError('PRIVATE') if fault == 'authentication-readiness' else None)
                c.tun.collect_preflight.return_value = {'systemNamespaces': {}, 'network': {}, 'vmPreflight': vm_tun.vm_observation()}
                obs=c.tun.collect_preflight.return_value['vmPreflight']
                obs['stages']={name:{'state':'observed','error':'none'} for name in vm_tun.VM_STAGES}
                obs['checks']=dict.fromkeys(vm_tun.VM_CHECKS,True);obs['managerControlGroup']='empty-root';obs['forwarding']=[0,0]
                c.tun.preflight.return_value = {'blocked': ['realPkexecAuthenticationPendingConfirmation']}
                if fault == 'vm-preflight': c.tun.preflight.return_value = {'blocked': ['rootManager']}
                c.tun.file_identity.side_effect = ValueError('PRIVATE') if fault == 'tool-identities' else None
                c.tun.prepare_root.return_value = ledger
                if fault == 'root-ledger': c.tun.prepare_root.side_effect = ValueError('PRIVATE')
                if fault == 'controller-ledger': ledger.mutate = Mock(side_effect=ValueError('PRIVATE'))
                if fault == 'auth-watchdog-start': c.ops.run.side_effect = ValueError('PRIVATE')
                c.tun.cleanup.return_value = {'state': 'failed'}
                with patch.object(m, 'independent_record'), patch.object(m, 'verify_timer',
                    side_effect=ValueError('PRIVATE') if fault == 'auth-watchdog-verify' else None):
                    with self.assertRaises(ValueError): c.prepare()
                self.assertEqual(c.stage, fault)
                with patch.object(m, 'close_auth'), patch.object(m, 'close_transactions'):
                    recovery = c.abort()
                self.assertEqual(recovery['state'], 'failed')
                if fault == 'root-ledger':
                    self.assertTrue(c.preparation_pending)
                    self.assertIn('root-preparation-incomplete', recovery['unknown'])
                    c.tun.cleanup.assert_not_called()  # Never adopt an unconfirmed ledger.
                elif fault in ('controller-ledger', 'auth-watchdog-start', 'auth-watchdog-verify'):
                    c.tun.cleanup.assert_called_once_with(ledger, forced=True)
                else: c.tun.prepare_root.assert_not_called()
                self.assertNotIn('PRIVATE', json.dumps(recovery))


class ExistingGroupObservationTests(unittest.TestCase):
    def user(self):
        return types.SimpleNamespace(pw_uid=1001, pw_name='runner', pw_gid=42,
                                     pw_shell='/bin/bash', pw_dir='/home/runner')

    def test_original_primary_or_explicit_membership_only(self):
        for name in ('sudo', 'admin', 'adm', 'docker', 'sudo-extra'):
            for primary, supplementary in ((True, False), (False, True), (False, False)):
                row = types.SimpleNamespace(gr_name=name, gr_gid=42 if primary else 99,
                                            gr_mem=['runner'] if supplementary else [])
                with self.subTest(name=name, primary=primary, supplementary=supplementary), patch('grp.getgrall', return_value=[row]):
                    value = m.validate_group_observation(m.observe_existing_admin_groups(self.user()))
                    self.assertEqual(value['groupRequirementSatisfied'], name in ('sudo','admin') and (primary or supplementary))
                    self.assertEqual(set(value['groups']), {'sudo','admin'})

    def test_errors_are_bounded_and_never_echo_nss(self):
        for error, label in ((OSError('PRIVATE'), 'OSError'), (KeyError('PRIVATE'),'KeyError'),
                             (TypeError('PRIVATE'),'TypeError'), (RuntimeError('PRIVATE'),'other')):
            with patch('grp.getgrall', side_effect=error):
                value = m.validate_group_observation(m.observe_existing_admin_groups(self.user()))
            self.assertEqual(value['errorType'], label)
            self.assertEqual(value['guard'], 'account-groups-enumerate')
            self.assertNotIn('PRIVATE', json.dumps(value))
            self.assertIsNone(value['groupRequirementSatisfied'])

    def test_recovery_never_enumerates_groups_but_keeps_exact_account(self):
        c = controller(m.AUTH_MODE); user = self.user()
        with patch.object(m.pwd,'getpwall',return_value=[user]), patch.object(m.pwd,'getpwnam',return_value=user), \
             patch('grp.getgrall',side_effect=AssertionError('unused group lookup')) as groups, \
             patch.object(m,'account_status',return_value='P'):
            self.assertEqual(m.credential_account(c.launch,c.tun,recovery=True)[0], user)
            groups.assert_not_called()
            user.pw_uid = 1002
            with self.assertRaises(ValueError): m.credential_account(c.launch,c.tun,recovery=True)
            groups.assert_not_called()

    def test_forged_group_receipts_rejected(self):
        with patch('grp.getgrall',return_value=[]): value=m.observe_existing_admin_groups(self.user())
        for change in ({'errorType':'PRIVATE'}, {'groupRequirementSatisfied':True}, {'extra':True}):
            with self.assertRaises(ValueError): m.validate_group_observation(value | change)



def group_controller():
    c = controller(m.AUTH_MODE)
    c.launch['authenticationMode'] = m.AUTH_GROUP_MODE
    c.launch['authorization'].update(schema='polaris-tun-credential-group-lease-v1', phrase=m.AUTH_GROUP_PHRASE,
        authMode=m.AUTH_GROUP_MODE, groupLease={'group':'sudo','account':'runner','guiUid':1001,
        'maxLeaseSeconds':90,'originalMembership':'absent','restore':'remove-only-added-membership'})
    c.group_binding = {'account':'runner','guiUid':1001,'primaryGid':1001,'group':'sudo','groupGid':451}
    return c


class GroupLeaseAuthorityTests(unittest.TestCase):
    def test_new_authority_is_separate_and_fixed(self):
        c=group_controller();m.validate_launch(c.launch)
        for key,value in (('phrase',m.AUTH_PHRASE),('schema','polaris-tun-credential-lease-v1'),
                          ('authMode',m.AUTH_MODE),('account','root'),('guiUid',1002)):
            bad=copy.deepcopy(c.launch);bad['authorization'][key]=value
            with self.subTest(key=key),self.assertRaises(ValueError):m.validate_permission(bad)
        for key,value in (('group','admin'),('maxLeaseSeconds',91),('guiUid',1002),('restore','replace-groups'),('originalMembership','present')):
            bad=copy.deepcopy(c.launch);bad['authorization']['groupLease'][key]=value
            with self.subTest(key=key),self.assertRaises(ValueError):m.validate_permission(bad)
        old=launch(m.AUTH_MODE);old['authorization']['groupLease']=c.launch['authorization']['groupLease']
        with self.assertRaises(ValueError):m.validate_permission(old)

    def test_group_binding_fixed_account_gid_absence(self):
        c=group_controller();user=types.SimpleNamespace(pw_uid=1001,pw_name='runner',pw_gid=1001)
        group=types.SimpleNamespace(gr_name='sudo',gr_gid=451,gr_mem=[])
        with patch.object(m.pwd,'getpwnam',return_value=user),patch.object(m.pwd,'getpwall',return_value=[user]),patch('grp.getgrnam',return_value=group):
            self.assertEqual(m.group_binding(c.launch,expected_member=False)[0],c.group_binding)
            group.gr_mem=['runner']
            with self.assertRaises(ValueError):m.group_binding(c.launch,expected_member=False)
            group.gr_mem=[];group.gr_gid=452
            with self.assertRaises(ValueError):m.group_binding(c.launch,original=c.group_binding)
            group.gr_gid=451;user.pw_uid=1002
            with self.assertRaises(ValueError):m.group_binding(c.launch)

    def test_pending_group_does_not_authorize_old_mode(self):
        with self.assertRaises(ValueError):m.credential_account(launch(m.AUTH_MODE),Mock(),group_pending=True)

    def test_group_mode_pending_still_rejects_existing_password(self):
        c=group_controller();user=types.SimpleNamespace(pw_uid=1001,pw_name='runner',pw_gid=1001,pw_shell='/bin/bash',pw_dir='/home/runner')
        with patch.object(m.pwd,'getpwall',return_value=[user]),patch.object(m.pwd,'getpwnam',return_value=user), \
             patch('grp.getgrall',return_value=[]),patch.object(m,'group_binding',return_value=(c.group_binding,False)), \
             patch.object(m,'account_status',return_value='P'),patch.object(m.subprocess,'Popen') as spawn:
            with self.assertRaises(ValueError):m.credential_account(c.launch,c.tun,group_pending=True)
            spawn.assert_not_called()



def credential_proof():
    return {'account':'runner','guiUid':1001,'finalPasswordStatus':'L','emptyPasswordRestored':False,
            'plaintextPersisted':False,'temporaryPasswordHash':'locked until disposable VM destruction'}

def membership_proof():
    return {'schema':'polaris-sudo-membership-recovery-v1','account':'runner','guiUid':1001,
            'group':'sudo','groupGid':451,'originalMembership':'absent','finalMembership':'absent',
            'removedOnlyAddedMembership':True,'writerStopped':True,'maxLeaseSeconds':90}

class GroupLeaseRecoveryTests(unittest.TestCase):
    def lease(self):
        c=group_controller();c.auth_reached('lease-account','install');lease=m.CredentialLease(c,'install');c.lease=lease
        lease.attempted=True;lease.group_intent_written=True;lease.group_timer_verified=True
        c.ledger.value['authentication']['lease']={};c.ops.run.return_value='inactive'
        return c,lease

    def test_relock_remove_independent_and_failure_keeps_timer(self):
        for fail in ('lock','remove','lineage'):
            c,lease=self.lease();events=[];c.auth_observer=Mock()
            c.auth_observer.close.side_effect=ValueError('PRIVATE') if fail=='lineage' else lambda:events.append('lineage') or {'ownedBirths':[],'unknownAdoptedChildren':[],'forced':False}
            def lock(*a):
                events.append('lock')
                if fail=='lock':raise ValueError('PRIVATE')
                return credential_proof()
            def remove(*a):
                events.append('remove')
                if fail=='remove':raise ValueError('PRIVATE')
                return membership_proof()
            with patch.object(m,'relock_account',side_effect=lock),patch.object(m,'recover_group_membership',side_effect=remove):
                with self.subTest(fail=fail),self.assertRaises(ValueError):lease.close()
            self.assertLess(events.index('lock'),events.index('remove'));c.ops.run.assert_not_called()
            self.assertFalse(lease.closed);self.assertTrue(c.recovery_unknown)
            self.assertNotIn('PRIVATE',json.dumps(c.recovery_unknown))

    def test_success_closes_timer_only_after_both_proofs(self):
        c,lease=self.lease();events=[]
        with patch.object(m,'relock_account',side_effect=lambda *a:events.append('lock') or credential_proof()), \
             patch.object(m,'recover_group_membership',side_effect=lambda *a:events.append('remove') or membership_proof()), \
             patch.object(m,'read_group_intent',return_value={'binding':c.group_binding}),patch.object(m,'write_group_closed'):
            lease.close()
        self.assertEqual(events,['lock','remove']);self.assertTrue(lease.closed)
        self.assertEqual(c.ledger.value['authentication']['lease']['groupRecovery'],membership_proof())
        self.assertEqual(c.ops.run.call_args_list[0].args[0][1],'stop')

    def test_group_restore_without_password_intent_does_not_lock(self):
        c,lease=self.lease();lease.attempted=False
        with patch.object(m,'relock_account') as lock,patch.object(m,'recover_group_membership',return_value=membership_proof()) as remove:
            lease.close();lock.assert_not_called();remove.assert_called_once()

    def test_missing_writer_evidence_cannot_be_clean(self):
        c=group_controller()
        with patch.object(m,'read_group_intent',return_value={'binding':c.group_binding}), \
             patch.object(m,'stop_group_writer',return_value=False),patch.object(m,'run_group_writer') as mutate:
            with self.assertRaises(ValueError):m.recover_group_membership(c.launch,c.tun,c.directory,'install',c.ops)
            mutate.assert_not_called()

    def test_exact_remove_postcondition_not_exit_code(self):
        c=group_controller()
        with patch.object(m,'read_group_intent',return_value={'binding':c.group_binding}), \
             patch.object(m,'stop_group_writer',side_effect=[True,False,True]), \
             patch.object(m,'group_binding',side_effect=[(c.group_binding,True),ValueError('still member')]), \
             patch.object(m,'run_group_writer',return_value=True) as writer:
            with self.assertRaises(ValueError):m.recover_group_membership(c.launch,c.tun,c.directory,'install',c.ops)
            self.assertEqual(writer.call_args.args[-2],'remove')

    def test_old_close_never_reads_or_writes_groups(self):
        c=controller(m.AUTH_MODE);c.auth_reached('lease-account','install');lease=m.CredentialLease(c,'install');c.ops.run.return_value='inactive'
        with patch.object(m,'group_binding') as binding,patch.object(m,'run_group_writer') as writer,patch.object(m,'recover_group_membership') as recover:
            lease.close();binding.assert_not_called();writer.assert_not_called();recover.assert_not_called()


class FixedGroupWriterTests(unittest.TestCase):
    def test_parent_never_releases_gate_before_durable_birth(self):
        c=group_controller();events=[];row=identity(80,0);row['ppid']=os.getpid()
        c.ops.proc.return_value=row;c.tun.file_identity.return_value={'dev':1,'inode':77,'sha256':H}
        c.tun.executable_identity.return_value=c.tun.file_identity.return_value
        child=Mock(pid=80,returncode=0);child.communicate.side_effect=lambda **kw:events.append(('gate',kw['input']))
        with patch.object(m,'read_group_intent',return_value={'binding':c.group_binding}), \
             patch.object(m.subprocess,'Popen',return_value=child), \
             patch.object(m,'independent_record',side_effect=lambda *a:events.append(('record',a[1]))), \
             patch.object(m,'group_binding',return_value=(c.group_binding,True)):
            m.run_group_writer(c.launch,c.tun,c.directory,'install','add',c.ops)
        self.assertEqual(events,[('record','group-writer-install-add.json'),('gate',b'GO\n')])
        self.assertNotIn('/usr/bin/gpasswd',child.call_args_list if child.call_args_list else [])

    def test_pre_gate_birth_record_failure_closes_input_no_go(self):
        for fault in ('birth','record'):
            c=group_controller();row=identity(80,0);row['ppid']=os.getpid();c.ops.proc.return_value=None if fault=='birth' else row
            c.tun.file_identity.return_value={'dev':1,'inode':77,'sha256':H};c.tun.executable_identity.return_value=c.tun.file_identity.return_value
            child=Mock(pid=80,returncode=None);child.poll.return_value=None
            with patch.object(m,'read_group_intent',return_value={'binding':c.group_binding}),patch.object(m.subprocess,'Popen',return_value=child), \
                 patch.object(m,'independent_record',side_effect=ValueError('record failed') if fault=='record' else None),patch.object(m,'stop_group_writer',return_value=False):
                with self.subTest(fault=fault),self.assertRaises(ValueError):m.run_group_writer(c.launch,c.tun,c.directory,'install','add',c.ops)
            child.communicate.assert_not_called();child.stdin.close.assert_called_once()

    def test_child_eof_malformed_or_deadline_never_executes(self):
        c=group_controller();row=identity(os.getpid(),0);record={'runId':c.launch['runId'],'kind':'install','operation':'add','identity':row,'python':{},'gpasswd':{}}
        c.tun.process_identity.return_value=row;c.tun.executable_identity.return_value={};c.tun.file_identity.return_value={}
        for raw in (b'',b'NO\n',b'GO\n'):
            with patch.object(m,'read_group_intent',return_value={'binding':c.group_binding,'deadlineMonotonic':0}), \
                 patch.object(m,'read_line',return_value=raw),patch.object(m.Path,'read_bytes',return_value=json.dumps(record).encode()), \
                 patch.object(m,'group_binding',return_value=(c.group_binding,False)),patch.object(m.os,'execve') as execute:
                with self.subTest(raw=raw),self.assertRaises(ValueError):m.group_writer_main(c.launch,c.tun,c.directory,'install','add')
                execute.assert_not_called()

    def test_child_same_birth_exec_only_fixed_command(self):
        c=group_controller();row=identity(os.getpid(),0);record={'runId':c.launch['runId'],'kind':'install','operation':'add','identity':row,'python':{},'gpasswd':{}}
        c.tun.process_identity.return_value=row;c.tun.executable_identity.return_value={};c.tun.file_identity.return_value={}
        with patch.object(m,'read_group_intent',return_value={'binding':c.group_binding,'deadlineMonotonic':time.monotonic()+85}), \
             patch.object(m,'read_line',return_value=b'GO\n'),patch.object(m.Path,'read_bytes',return_value=json.dumps(record).encode()), \
             patch.object(m,'group_binding',return_value=(c.group_binding,False)),patch.object(m.os,'execve') as execute,patch.object(m.subprocess,'Popen') as spawn:
            m.group_writer_main(c.launch,c.tun,c.directory,'install','add')
            execute.assert_called_once_with('/usr/bin/gpasswd',['/usr/bin/gpasswd','-a','runner','sudo'],m.ENV)
            spawn.assert_not_called()

    def test_recovery_rejects_different_birth_or_arbitrary_root_exe(self):
        for fault in ('birth','exe'):
            c=group_controller();old=identity(80,0);now=copy.deepcopy(old)
            if fault=='birth':now['startTime']='999'
            c.ops.proc.return_value=now;c.tun.file_identity.return_value={'dev':1,'inode':2,'sha256':H}
            c.tun.executable_identity.return_value={'dev':1,'inode':3 if fault=='exe' else 2,'sha256':H}
            record={'runId':c.launch['runId'],'kind':'install','operation':'add','identity':old,
                    'python':c.tun.file_identity.return_value,'gpasswd':c.tun.file_identity.return_value}
            with patch.object(m.Path,'exists',return_value=True),patch.object(m.Path,'stat',return_value=types.SimpleNamespace(st_size=500)), \
                 patch.object(m.Path,'read_bytes',return_value=json.dumps(record).encode()),patch.object(m,'stop_recorded') as stop:
                with self.subTest(fault=fault),self.assertRaises(ValueError):m.stop_group_writer(c.launch,c.tun,c.directory,'install','add',c.ops)
                stop.assert_not_called()



class JointLeaseOrderingTests(unittest.TestCase):
    def exercise_open(self, fault=None):
        c=group_controller();lease=m.CredentialLease(c,'install');c.agent=Mock();c.ops.run.return_value='inactive'
        c.ledger.value['authentication']['registered']=True;events=[]
        writer=Mock(pid=89,returncode=0);writer.poll.return_value=0
        c.ops.proc.return_value={**identity(89,0),'ppid':os.getpid()}
        c.tun.executable_identity.return_value=c.tun.file_identity.return_value={'sha256':H}
        writer.communicate.side_effect=lambda **kw:events.append('password-input')
        def point(name):
            def f(*args,**kw):
                events.append(name)
                if fault==name:raise ValueError('PRIVATE')
                if name=='verified-timer':return {'deadlineMonotonic':time.monotonic()+10}
            return f
        user=types.SimpleNamespace(pw_name='runner',pw_uid=1001)
        account=Mock(return_value=(user,'L'))
        with patch.object(m,'credential_account',account),patch.object(m,'group_binding',return_value=(c.group_binding,False)), \
             patch.object(m,'group_intent',side_effect=point('group-intent')),patch.object(m,'verify_timer',side_effect=point('verified-timer')), \
             patch.object(m,'run_group_writer',side_effect=point('group-add')),patch.object(m,'account_status',side_effect=['L','P']), \
             patch.object(m,'setting_intent',side_effect=point('password-intent')),patch.object(m,'independent_record',side_effect=point('password-birth')), \
             patch.object(m.subprocess,'Popen',return_value=writer) as spawn,patch.object(m.os,'pidfd_open',return_value=99), \
             patch.object(m.threading,'Thread',return_value=Mock()),patch.object(lease,'close'):
            if fault:
                with self.assertRaises(ValueError):lease.open()
            else:lease.open()
        return c,lease,events,spawn,account

    def test_order_intent_timer_add_original_gate_then_password(self):
        c,lease,events,spawn,account=self.exercise_open()
        self.assertEqual(events,['group-intent','verified-timer','group-add','password-intent','password-birth','password-input'])
        self.assertEqual(account.call_args_list[0].kwargs,{'group_pending':True})
        self.assertEqual(account.call_args_list[1].kwargs,{})
        self.assertLessEqual(lease.group_deadline-time.monotonic(),90)
        self.assertEqual(spawn.call_count,1)

    def test_no_password_writer_before_every_group_guard(self):
        for fault in ('group-intent','verified-timer','group-add'):
            with self.subTest(fault=fault):
                c,lease,events,spawn,account=self.exercise_open(fault)
                spawn.assert_not_called();self.assertNotIn('password-intent',events)
                if fault!='group-add':self.assertNotIn('group-add',events)

    def test_pending_readiness_never_claims_authenticated_or_group_present(self):
        c=group_controller();user=types.SimpleNamespace(pw_name='runner',pw_uid=1001)
        with patch.object(m,'group_binding',return_value=(c.group_binding,False)), \
             patch.object(m,'credential_account',return_value=(user,'L')) as account:
            result=c.authentication_readiness()
            self.assertFalse(result['authenticated']);self.assertEqual(result['groupMembership'],'pending-guarded-lease')
            self.assertTrue(account.call_args.kwargs['group_pending'])

    def test_private_pump_does_not_send_for_changed_selected_identity(self):
        c=group_controller();c.pty=90;lease=m.CredentialLease(c,'install');lease.window=90;lease.group_deadline=time.monotonic()+90
        lease.user=types.SimpleNamespace(pw_name='runner',pw_gecos='runner');lease.secret=bytearray(b'PRIVATE')
        chunks=[b'==== AUTHENTICATING FOR org.freedesktop.policykit.exec ====\nChoose identity to authenticate as\n 1.  runner\n',
                b'Authenticating as: root\nPassword:']
        with patch.object(m.select,'select',return_value=([90],[],[])),patch.object(m.os,'read',side_effect=chunks), \
             patch.object(m.os,'write') as write,patch.object(lease,'close'):
            lease.pump()
        self.assertFalse(lease.sent);self.assertFalse(lease.authenticated)
        self.assertEqual(write.call_args_list,[unittest.mock.call(90,b'1\n')])


class JointWatchdogTests(unittest.TestCase):
    def run_watchdog(self, fault):
        c=group_controller();events=[];c.tun.Ledger.side_effect=ValueError('PRIVATE ledger')
        with tempfile.TemporaryDirectory() as d:
            directory=Path(d);(directory/'controller-birth.json').write_text(json.dumps(identity(81,0)))
            def point(name,result=None):
                def f(*args,**kwargs):
                    events.append(name)
                    if fault==name:raise ValueError('PRIVATE')
                    return result
                return f
            with patch.object(m,'has_setting_intent',return_value=fault!='no-password'), \
                 patch.object(m,'stop_recorded',side_effect=point('producer-stop')), \
                 patch.object(m,'read_group_intent',side_effect=point('group-intent',{})), \
                 patch.object(m,'stop_group_writer',side_effect=point('group-writer-stop',True)), \
                 patch.object(m,'relock_account',side_effect=point('relock',{'finalPasswordStatus':'L'})), \
                 patch.object(m,'recover_group_membership',side_effect=point('group-remove',{'finalMembership':'absent'})):
                result=m.watchdog_recover(c.launch,c.tun,directory,'install')
                receipt=json.loads((directory/'emergency-install.json').read_text())
        return events,result,receipt

    def test_watchdog_both_restorations_survive_ledger_and_each_other_failure(self):
        for fault in ('relock','group-remove','group-writer-stop','no-password'):
            with self.subTest(fault=fault):
                events,result,receipt=self.run_watchdog(fault)
                self.assertEqual(events[0],'producer-stop');self.assertIn('group-remove',events)
                self.assertEqual('relock' in events,fault!='no-password')
                self.assertLess(events.index('group-writer-stop'),events.index('group-remove'))
                self.assertEqual(result['result'],'failed');self.assertFalse(receipt['restorationProven'])
                self.assertIn('product-ledger-recovery-incomplete',result['errors'])
                self.assertNotIn('PRIVATE',json.dumps(receipt))

    def test_group_intent_invalid_never_mutates_group(self):
        events,result,_=self.run_watchdog('group-intent')
        self.assertIn('group-intent-unproved',result['errors']);self.assertNotIn('group-remove',events)
        self.assertIn('group-writer-stop',events) # recorded writer stopped even if intent cannot be parsed

    def test_intent_persists_before_writer_and_rejects_changed_permission(self):
        c=group_controller()
        with tempfile.TemporaryDirectory() as d,patch.object(m,'group_binding',return_value=(c.group_binding,False)):
            directory=Path(d);m.group_intent(directory,'install',c.launch,c.group_binding,time.monotonic()+60)
            value=m.read_group_intent(directory,'install',c.launch,c.tun)
            self.assertEqual(value['binding']['groupGid'],451)
            self.assertEqual((directory/'group-intent-install.json').stat().st_mode & 0o777,0o600)
            c.launch['authorization']['expiresUnix']+=1
            with self.assertRaises(ValueError):m.read_group_intent(directory,'install',c.launch,c.tun)



class GroupWriterFinalBoundaryTests(unittest.TestCase):
    def test_exec_transition_race_never_claims_stopped(self):
        c=group_controller();old=identity(80,0);c.ops.proc.return_value=old
        executable={'dev':1,'inode':77,'sha256':H};c.tun.file_identity.return_value=executable;c.tun.executable_identity.return_value=executable
        record={'runId':c.launch['runId'],'kind':'install','operation':'add','identity':old,'python':executable,'gpasswd':executable}
        with patch.object(m.Path,'exists',return_value=True),patch.object(m.Path,'stat',return_value=types.SimpleNamespace(st_size=500)), \
             patch.object(m.Path,'read_bytes',return_value=json.dumps(record).encode()), \
             patch.object(m,'stop_recorded',side_effect=ValueError('exec changed during signal gate')):
            with self.assertRaises(ValueError):m.stop_group_writer(c.launch,c.tun,c.directory,'install','add',c.ops)

    def test_remove_timeout_or_collision_never_reports_absence(self):
        for fault in ('timeout','collision','exit','reap'):
            c=group_controller();row={**identity(80,0),'ppid':os.getpid()};c.ops.proc.return_value=row
            c.tun.file_identity.return_value=c.tun.executable_identity.return_value={'dev':1,'inode':77,'sha256':H}
            child=Mock(pid=80,returncode=1 if fault=='exit' else 0);child.poll.return_value=None
            if fault in ('timeout','reap'):child.communicate.side_effect=subprocess.TimeoutExpired('fixed',10)
            if fault=='reap':child.wait.side_effect=subprocess.TimeoutExpired('fixed',5)
            with patch.object(m,'read_group_intent',return_value={'binding':c.group_binding}), \
                 patch.object(m.subprocess,'Popen',return_value=child), \
                 patch.object(m,'independent_record',side_effect=FileExistsError() if fault=='collision' else None), \
                 patch.object(m,'stop_group_writer',return_value=True) as stop,patch.object(m,'group_binding') as binding:
                with self.subTest(fault=fault),self.assertRaises(Exception):m.run_group_writer(c.launch,c.tun,c.directory,'install','remove',c.ops)
                binding.assert_not_called();stop.assert_called_once();child.wait.assert_called_once_with(timeout=5)
                if fault=='collision':child.communicate.assert_not_called()

    def test_unknown_previous_removal_does_not_spawn_unrecorded_retry(self):
        c=group_controller()
        with patch.object(m,'read_group_intent',return_value={'binding':c.group_binding}), \
             patch.object(m,'stop_group_writer',return_value=True),patch.object(m,'group_binding',return_value=(c.group_binding,True)), \
             patch.object(m,'run_group_writer') as writer:
            with self.assertRaises(ValueError):m.recover_group_membership(c.launch,c.tun,c.directory,'install',c.ops)
            writer.assert_not_called()

    def test_cross_transaction_gid_change_rejected_before_intent_or_writer(self):
        c=group_controller();c.agent=Mock();c.ledger.value['authentication']['registered']=True
        lease=m.CredentialLease(c,'uninstall');user=types.SimpleNamespace(pw_name='runner',pw_uid=1001)
        with patch.object(m,'credential_account',return_value=(user,'L')), \
             patch.object(m,'group_binding',side_effect=ValueError('GID changed')) as binding, \
             patch.object(m,'read_group_closed',return_value={}),patch.object(m,'group_intent') as intent,patch.object(m,'run_group_writer') as writer,patch.object(lease,'close'):
            with self.assertRaises(ValueError):lease.open()
            self.assertEqual(binding.call_args.kwargs['original'],c.group_binding)
            intent.assert_not_called();writer.assert_not_called()

    def test_add_completed_before_parent_loss_still_requires_remove_fence_and_absence(self):
        c=group_controller();events=[]
        stop_results=iter((True,False,True))
        def stopped(*args):events.append('stop-'+args[-2]);return next(stop_results)
        def binding(*args,**kw):events.append('absence' if kw.get('expected_member') is False else 'membership');return c.group_binding,True
        with patch.object(m,'read_group_intent',return_value={'binding':c.group_binding}),patch.object(m,'stop_group_writer',side_effect=stopped), \
             patch.object(m,'group_binding',side_effect=binding),patch.object(m,'run_group_writer',side_effect=lambda *a:events.append('remove')):
            result=m.recover_group_membership(c.launch,c.tun,c.directory,'install',c.ops)
        self.assertEqual(events,['stop-add','stop-remove','membership','remove','stop-remove','absence'])
        self.assertTrue(result['writerStopped']);self.assertEqual(result['groupGid'],451)



class FixedJointBudgetTests(unittest.TestCase):
    def test_twenty_activity_seventy_reserved_and_both_service_budgets(self):
        self.assertEqual((m.GROUP_ACTIVITY_SECONDS,m.GROUP_RECOVERY_RESERVE),(20,70))
        _,lease,_,_,_=JointLeaseOrderingTests().exercise_open()
        # Source contract complements mocks: both actual systemd producers use
        # verified new-mode recovery budget; old20/15 values remain unchanged.
        source=Path(m.__file__).read_text()
        self.assertIn("timer_seconds = GROUP_ACTIVITY_SECONDS if self.grouped else window",source)
        self.assertIn("GROUP_RECOVERY_SERVICE_SECONDS if self.grouped else 20",source)
        self.assertIn("GROUP_RECOVERY_SERVICE_SECONDS if self.launch['authenticationMode'] == AUTH_GROUP_MODE else 15",source)
        self.assertEqual(source.count("{'recovery_seconds': GROUP_RECOVERY_SERVICE_SECONDS}"),2)

    def test_actual_service_budget_is_verified_not_assumed(self):
        argv=['/usr/bin/env','fixed'];unit='fixed-unit'
        for budget in ('20s','70s','infinity'):
            service='\n'.join(['User=root','Group=root','DynamicUser=no','FragmentPath=/run/systemd/transient/fixed-unit.service',
                'DropInPaths=','ActiveState=inactive','ExecStart={ path=/usr/bin/env ; argv[]=/usr/bin/env fixed ; }','TimeoutStartUSec='+budget])
            timer='ActiveState=active\nUnit=fixed-unit.service\nTimersMonotonic=[ { OnActiveUSec=20s ; next_elapse=120s } ]'
            ops=Mock();ops.run.side_effect=[service,timer]
            with patch.object(m.time,'monotonic',return_value=100):
                if budget=='70s':m.verify_timer(ops,Mock(SYSTEMCTL='/usr/bin/systemctl'),unit,argv,20,100,recovery_seconds=70)
                else:
                    with self.assertRaises(ValueError):m.verify_timer(ops,Mock(SYSTEMCTL='/usr/bin/systemctl'),unit,argv,20,100,recovery_seconds=70)

    def test_overdue_normal_recovery_never_cancels_timer_or_writes_closed(self):
        c,lease=GroupLeaseRecoveryTests().lease();lease.group_deadline=time.monotonic()-1
        with patch.object(m,'relock_account',return_value=credential_proof()), \
             patch.object(m,'recover_group_membership',return_value={'finalMembership':'absent'}),patch.object(m,'write_group_closed') as closed:
            with self.assertRaises(ValueError):lease.close()
        self.assertIn('joint-lease-deadline-exceeded',c.recovery_unknown)
        self.assertFalse(lease.closed);closed.assert_not_called();c.ops.run.assert_not_called()

    def test_group_relock_skips_unused_prelock_status_but_keeps_exact_uid(self):
        c=group_controller();user=types.SimpleNamespace(pw_uid=1001,pw_name='runner')
        with patch.object(m.pwd,'getpwall',return_value=[user]),patch.object(m.pwd,'getpwnam',return_value=user), \
             patch.object(m,'account_status',return_value='L') as status,patch.object(m.subprocess,'run') as run:
            m.relock_account(c.launch,c.tun)
            status.assert_called_once_with('runner');self.assertEqual(run.call_args.kwargs['timeout'],10)
            self.assertEqual(run.call_args.args[0],['/usr/bin/passwd','-l','runner'])
            user.pw_uid=1002
            with self.assertRaises(ValueError):m.relock_account(c.launch,c.tun)
            self.assertEqual(run.call_count,1)

    def test_immediate_emergency_fence_still_exact_identity(self):
        c=group_controller();old=identity(80,0);c.ops.proc.return_value=old;c.ops.signal_owned.return_value=True
        self.assertTrue(m.stop_recorded(c.ops,c.tun,old,immediate=True))
        c.ops.signal_owned.assert_called_once_with(old,signal.SIGKILL)
        c.ops.proc.return_value={**old,'startTime':'999'};c.ops.signal_owned.reset_mock()
        with self.assertRaises(ValueError):m.stop_recorded(c.ops,c.tun,old,immediate=True)
        c.ops.signal_owned.assert_not_called()

    def test_agent_fenced_before_restore_and_account_checkpoint_before_lineage(self):
        c=group_controller();events=[];c.tun.Ledger.side_effect=ValueError('ledger')
        with tempfile.TemporaryDirectory() as d:
            directory=Path(d)
            for name,pid in (('controller-birth.json',81),('agent-install.json',82)):
                (directory/name).write_text(json.dumps(identity(pid,0 if pid==81 else 1001)))
            def stop(ops,tun,old,**kw):
                events.append(('stop',old['pid'],kw.get('immediate',False)))
                if old['pid']==82 and not kw.get('immediate'):
                    self.assertTrue((directory/'emergency-install-account.json').exists())
            with patch.object(m,'read_group_closed',return_value=None),patch.object(m,'has_setting_intent',return_value=True), \
                 patch.object(m,'read_group_intent',return_value={'deadlineMonotonic':time.monotonic()+60}), \
                 patch.object(m,'stop_recorded',side_effect=stop),patch.object(m,'stop_group_writer',return_value=True), \
                 patch.object(m,'relock_account',side_effect=lambda *a:events.append(('lock',)) or {}), \
                 patch.object(m,'recover_group_membership',side_effect=lambda *a:events.append(('remove',)) or {}):
                result=m.watchdog_recover(c.launch,c.tun,directory,'install')
            self.assertLess(events.index(('stop',82,True)),events.index(('lock',)))
            self.assertLess(events.index(('remove',)),events.index(('stop',82,False)))
            self.assertEqual(result['result'],'failed')

    def test_durable_closed_receipt_fresh_process_checks_and_no_ledger_authority(self):
        c=group_controller();credential={'account':'runner','guiUid':1001,'finalPasswordStatus':'L',
            'emptyPasswordRestored':False,'plaintextPersisted':False,'temporaryPasswordHash':'locked until disposable VM destruction'}
        group={'schema':'polaris-sudo-membership-recovery-v1','account':'runner','guiUid':1001,
            'group':'sudo','groupGid':451,'originalMembership':'absent','finalMembership':'absent',
            'removedOnlyAddedMembership':True,'writerStopped':True,'maxLeaseSeconds':90}
        with tempfile.TemporaryDirectory() as d,patch.object(m,'group_binding',return_value=(c.group_binding,False)),patch.object(m,'proc_absent',return_value=True) as absent:
            directory=Path(d);deadline=time.monotonic()+90
            m.group_intent(directory,'install',c.launch,c.group_binding,deadline)
            m.write_group_closed(directory,'install',c.launch,c.group_binding,deadline,credential,group)
            for name in ('writer-install.json','agent-install.json','group-writer-install-add.json','group-writer-install-remove.json'):
                (directory/name).write_text(json.dumps({'identity':identity(80,0)} if name.startswith('group-') else identity(80,0)))
            self.assertIsNotNone(m.read_group_closed(directory,'install',c.launch,c.tun))
            self.assertEqual(absent.call_count,4)
            absent.return_value=False
            with self.assertRaises(ValueError):m.read_group_closed(directory,'install',c.launch,c.tun)
            absent.return_value=True
            (directory/'writer-install.json').unlink()
            with self.assertRaises(FileNotFoundError):m.read_group_closed(directory,'install',c.launch,c.tun)

    def test_watch_auth_does_not_replay_independently_closed_install(self):
        c=group_controller();c.tun.Ledger.side_effect=ValueError('ledger')
        with tempfile.TemporaryDirectory() as d,patch.object(m,'stop_recorded'), \
             patch.object(m,'read_group_closed',side_effect=lambda directory,kind,*a: {} if kind=='install' else None), \
             patch.object(m,'read_group_intent',return_value={'deadlineMonotonic':time.monotonic()+80}), \
             patch.object(m,'has_setting_intent',return_value=True),patch.object(m,'stop_group_writer',return_value=True), \
             patch.object(m,'relock_account',return_value={}),patch.object(m,'recover_group_membership',return_value={}) as recover:
            directory=Path(d);(directory/'controller-birth.json').write_text(json.dumps(identity(81,0)))
            m.watchdog_recover(c.launch,c.tun,directory)
            recover.assert_called_once();self.assertEqual(recover.call_args.args[3],'uninstall')



class ActualJointTimerDeadlineTests(unittest.TestCase):
    def test_late_actual_target_rejected_before_add_even_with_valid_interval(self):
        c=group_controller();c.agent=Mock();c.ledger.value['authentication']['registered']=True
        lease=m.CredentialLease(c,'install');user=types.SimpleNamespace(pw_name='runner',pw_uid=1001)
        with patch.object(m.time,'monotonic',return_value=100),patch.object(m,'credential_account',return_value=(user,'L')), \
             patch.object(m,'group_binding',return_value=(c.group_binding,False)),patch.object(m,'group_intent'), \
             patch.object(m,'verify_timer',return_value={'deadlineMonotonic':121}) as verify, \
             patch.object(m,'run_group_writer') as writer,patch.object(lease,'close'):
            with self.assertRaises(ValueError):lease.open()
            writer.assert_not_called();self.assertFalse(lease.group_timer_verified)
            self.assertEqual(lease.group_deadline,190)
            self.assertEqual(verify.call_args.args[4],18) # 2 seconds arming/precision margin
            self.assertEqual(verify.call_args.kwargs['recovery_seconds'],70)

    def test_setup_time_consumes_activity_not_recovery_reserve(self):
        c=group_controller();c.agent=Mock();c.ledger.value['authentication']['registered']=True
        lease=m.CredentialLease(c,'install');user=types.SimpleNamespace(pw_name='runner',pw_uid=1001);clock=[100]
        def intent(*args):clock[0]=110
        with patch.object(m.time,'monotonic',side_effect=lambda:clock[0]),patch.object(m,'credential_account',return_value=(user,'L')), \
             patch.object(m,'group_binding',return_value=(c.group_binding,False)),patch.object(m,'group_intent',side_effect=intent), \
             patch.object(m,'verify_timer',return_value={'deadlineMonotonic':118}) as verify, \
             patch.object(m,'run_group_writer',side_effect=ValueError('stop test before mutation')) as writer,patch.object(lease,'close'):
            with self.assertRaises(ValueError):lease.open()
            self.assertEqual(lease.group_deadline,190) # never reset to200 after slow intent
            self.assertEqual(verify.call_args.args[4],8)
            self.assertTrue(lease.group_timer_verified);writer.assert_called_once()


class VMPreparationDiagnosticTests(unittest.TestCase):
    def fixture(self):
        c=controller(); c.ledger=None; c.authentication_readiness=Mock(return_value={})
        c.tun.collect_preflight.return_value={'vmPreflight':vm_tun.vm_observation()}
        return c

    def test_all_blocked_codes_preserved_without_root_mutation(self):
        for code in sorted(vm_tun.VM_BLOCKS-{'realPkexecAuthenticationPendingConfirmation'}):
            with self.subTest(code=code):
                c=self.fixture();c.tun.preflight.return_value={'blocked':sorted([code,'realPkexecAuthenticationPendingConfirmation'])}
                with patch.object(m,'independent_record') as record,patch.object(m,'CredentialLease') as lease:
                    with self.assertRaises(ValueError):c.prepare()
                self.assertEqual(c.vm_preflight['blocked'],sorted([code,'realPkexecAuthenticationPendingConfirmation']))
                self.assertEqual(c.vm_preflight['gate'],'blocked')
                c.tun.prepare_root.assert_not_called();c.tun.file_identity.assert_not_called()
                c.ops.run.assert_not_called();record.assert_not_called();lease.assert_not_called()
                receipt=c.abort();m.validate_abort_receipt(receipt,vm_validator=vm_tun.validate_vm_observation)
                self.assertFalse(receipt['prepared']);self.assertEqual(receipt['vmPreflight'],c.vm_preflight)
                self.assertEqual(receipt['state'],'failed')

    def test_gate_parser_exception_retains_stage_but_no_message_or_mutation(self):
        for error in (ValueError('PRIVATE'),TypeError('PRIVATE'),KeyError('PRIVATE')):
            c=self.fixture();c.tun.preflight.side_effect=error
            with self.assertRaises(type(error)):c.prepare()
            self.assertEqual(c.vm_preflight['gate'],'failed')
            self.assertEqual(c.vm_preflight['stages']['gate']['error'],type(error).__name__)
            self.assertNotIn('PRIVATE',json.dumps(c.abort()))
            c.tun.prepare_root.assert_not_called();c.ops.run.assert_not_called()

    def test_abort_optional_observation_cannot_bypass_validator(self):
        c=self.fixture();c.vm_preflight=vm_tun.vm_observation();r=c.abort()
        with self.assertRaises(ValueError):m.validate_abort_receipt(r)
        m.validate_abort_receipt(r,vm_validator=vm_tun.validate_vm_observation)
        r['vmPreflight']['checks']['rootManager']='PRIVATE'
        with self.assertRaises(ValueError):m.validate_abort_receipt(r,vm_validator=vm_tun.validate_vm_observation)

    def test_root_ipc_preserves_safe_observation(self):
        c=Mock();c.finished=False;c.seq=1;c.auth_snapshot.return_value=m.auth_runtime_observation('existing-session');c.readonly_auth=None;c.stage='vm-preflight'
        c.vm_preflight=vm_tun.vm_observation();c.vm_preflight['gate']='failed'
        c.vm_preflight['stages']['gate']={'state':'failed','error':'ValueError'}
        c.tun.validate_vm_observation=vm_tun.validate_vm_observation
        c.dispatch.side_effect=ValueError('PRIVATE');out=io.StringIO()
        import ctypes
        with patch.object(m.sys,'argv',['controller','serve','--run-id','0123456789ab']), \
             patch.object(m,'load_frozen',return_value=(launch(),Mock(),Path('/root/frozen'))), \
             patch.object(m,'Controller',return_value=c),patch.object(ctypes,'CDLL',return_value=Mock(prctl=Mock(return_value=0))), \
             patch.object(m,'read_line',side_effect=[b'{"seq":1,"action":"prepare"}',ValueError('EOF')]), \
             patch.object(m.sys,'stdout',out),self.assertRaises(ValueError):m.main()
        response=json.loads(out.getvalue().splitlines()[1])
        self.assertEqual(response['error']['vmPreflight'],c.vm_preflight)
        self.assertNotIn('PRIVATE',out.getvalue());c.abort.assert_called_once()


class AuthenticationRecoveryObservationTests(unittest.TestCase):
    def grouped(self):
        c=group_controller();c.auth_reached('password-writer','install')
        c.lease=m.CredentialLease(c,'install');c.lease.attempted=True
        c.lease.group_intent_written=True;c.lease.group_timer_verified=True
        c.ledger.value['authentication']['lease']={};c.ops.run.return_value='inactive'
        return c,c.lease

    def test_diagnostics_failure_never_blocks_lease_recovery(self):
        c=controller(m.AUTH_MODE);c.lease=Mock()
        with patch.object(c,'auth_failed',side_effect=ValueError('PRIVATE')):c.fail()
        c.lease.close.assert_called_once()
        self.assertIn('authentication-recovery-record-unknown',c.recovery_unknown)

    def test_partial_proofs_survive_lineage_failure_and_abort(self):
        c,l=self.grouped();c.auth_observer=Mock();c.auth_observer.close.side_effect=ValueError('PRIVATE')
        c.tun.cleanup.return_value={'state':'failed'}
        with patch.object(m,'relock_account',return_value=credential_proof()), \
             patch.object(m,'recover_group_membership',return_value=membership_proof()),patch.object(m,'close_transactions'):
            result=c.abort()
        observed=result['authenticationRuntime'];m.validate_auth_runtime_observation(observed)
        self.assertEqual(observed['credentialRecovery'],credential_proof())
        self.assertEqual(observed['groupRecovery'],membership_proof())
        self.assertEqual(result['state'],'failed');self.assertTrue(result['unknown'])
        self.assertEqual(observed['firstFailure'],{'stage':'password-writer','code':'abort-requested'})
        self.assertNotIn('PRIVATE',json.dumps(result));c.ops.run.assert_not_called()

    def test_late_writer_requires_fresh_relock_on_second_close(self):
        c,l=self.grouped();l.writer=Mock();l.writer.poll.return_value=None;l.writer_fd=77
        l.writer.wait.side_effect=[TimeoutError('PRIVATE'),0];l.group_deadline=time.monotonic()+80
        with patch.object(m.signal,'pidfd_send_signal'),patch.object(m.os,'close'), \
             patch.object(m,'relock_account',return_value=credential_proof()) as lock, \
             patch.object(m,'recover_group_membership',return_value=membership_proof()) as remove, \
             patch.object(m,'read_group_intent',return_value={'binding':c.group_binding}),patch.object(m,'write_group_closed'):
            with self.assertRaises(ValueError):l.close()
            # First lock/absence proof predates a possible late writer: never use it to skip work.
            l.writer_fd=78
            l.close()
        self.assertEqual(lock.call_count,2);self.assertEqual(remove.call_count,2)
        self.assertTrue(l.closed);self.assertIn('credential-writer-stop-unproved',c.recovery_unknown)

    def test_ledger_failure_keeps_independent_remove_and_timer(self):
        c,l=self.grouped();c.ledger.mutate=Mock(side_effect=OSError('PRIVATE'))
        with patch.object(m,'relock_account',return_value=credential_proof()) as lock, \
             patch.object(m,'recover_group_membership',return_value=membership_proof()) as remove:
            with self.assertRaises(ValueError):l.close()
        lock.assert_called_once();remove.assert_called_once();c.ops.run.assert_not_called()
        observed=c.auth_snapshot();self.assertEqual(observed['persistence'],'unknown')
        self.assertEqual(observed['groupRecovery'],membership_proof())

    def test_each_restoration_failure_does_not_skip_other(self):
        for broken in ('credential','group'):
            c,l=self.grouped()
            with self.subTest(broken=broken), \
                 patch.object(m,'relock_account',side_effect=ValueError('PRIVATE') if broken=='credential' else lambda *a:credential_proof()) as lock, \
                 patch.object(m,'recover_group_membership',side_effect=ValueError('PRIVATE') if broken=='group' else lambda *a:membership_proof()) as remove:
                with self.assertRaises(ValueError):l.close()
            lock.assert_called_once();remove.assert_called_once();c.ops.run.assert_not_called()
            self.assertEqual(c.auth_snapshot()['recovery'][broken],'unknown')

    def test_abort_observer_failure_still_attempts_exact_agent_auth_product(self):
        for broken in ('observer','retention'):
            c=controller(m.AUTH_MODE);c.auth_reached('lineage-start','install');events=[]
            c.auth_observer=Mock();c.auth_observer.close.return_value={'ownedBirths':[],'unknownAdoptedChildren':[],'forced':True}
            if broken=='observer':c.auth_observer.close.side_effect=ValueError('PRIVATE')
            c.tun.cleanup.side_effect=lambda *a,**kw:events.append('product') or {'state':'failed'}
            with self.subTest(broken=broken),patch.object(c,'retain_auth_lineage',side_effect=ValueError('PRIVATE')), \
                 patch.object(c,'fence_registered_agent',side_effect=lambda:events.append('fence')), \
                 patch.object(m,'close_auth',side_effect=lambda *a,**kw:events.append('auth')),patch.object(m,'close_transactions'):
                result=c.abort()
            self.assertEqual(events,['fence','auth','product']);self.assertTrue(result['unknown'])

    def test_initial_scan_failure_never_joins_unstarted_or_claims_complete(self):
        c=controller(m.AUTH_MODE);c.ledger.value['authentication']['agent']={'identity':identity(80)}
        observer=m.AuthObserver(c,'install')
        with patch.object(observer,'scan',side_effect=ValueError('PRIVATE')):
            with self.assertRaises(ValueError):observer.start()
        with patch.object(observer,'scan'),patch.object(observer.thread,'join') as join, \
             patch.object(m,'stop_recorded') as stop,patch.object(m.Path,'iterdir',return_value=[]):
            with self.assertRaises(ValueError):observer.close()
        join.assert_not_called();stop.assert_called_once();self.assertTrue(observer.stop.is_set())

    def test_intentional_agent_exit_not_natural_early_exit(self):
        for deliberate in (False,True):
            c=controller(m.AUTH_MODE);c.auth_reached('lineage-start','install');old=identity(80)
            c.agent=Mock(pid=80);c.agent.wait.return_value=-signal.SIGTERM
            c.ledger.value['authentication']['agent']={'identity':old}
            with patch.object(m,'stop_recorded',return_value=False),patch.object(m,'proc_absent',return_value=True):
                if deliberate:c.fence_registered_agent()
                if deliberate:m.close_auth(c.ledger,c.tun,forced=True)
                else:
                    with self.assertRaises(ValueError):m.close_auth(c.ledger,c.tun,forced=True)
            self.assertEqual('authentication-agent-exited-early' in c.ledger.value['unknown'],not deliberate)

    def test_first_failure_survives_abort_and_failed_persistence(self):
        c=controller(m.AUTH_MODE);c.auth_reached('agent-notify','install');c.auth_failed()
        c.lease=Mock();c.lease.close.side_effect=ValueError('PRIVATE');c.tun.cleanup.return_value={'state':'failed'}
        with patch.object(m,'close_transactions'):
            first=c.abort();second=c.abort()
        self.assertEqual(first['authenticationRuntime']['firstFailure'],second['authenticationRuntime']['firstFailure'])
        self.assertEqual(second['authenticationRuntime']['firstFailure']['stage'],'agent-notify')
        self.assertTrue(second['unknown']);self.assertEqual(second['state'],'failed')

    def test_last_timer_diagnostic_persistence_failure_permanently_blocks_finish(self):
        c,l=self.grouped();original=c.ledger.mutate
        def mutate(change):
            if c.auth_runtime['recovery']['timer']=='completed':raise OSError('PRIVATE')
            return original(change)
        c.ledger.mutate=mutate
        with patch.object(m,'relock_account',return_value=credential_proof()), \
             patch.object(m,'recover_group_membership',return_value=membership_proof()), \
             patch.object(m,'read_group_intent',return_value={'binding':c.group_binding}),patch.object(m,'write_group_closed'):
            l.close()
        self.assertTrue(l.closed);self.assertIn('authentication-recovery-record-unknown',c.recovery_unknown)
        c.ledger.value.update(phase='closed',forcedCleanup=False,unknown=[])
        c.ledger.value['authentication']['closed']=True
        with self.assertRaises(ValueError):c.finish()
        c.tun.cancel_watchdog.assert_not_called()
        with self.assertRaises(ValueError):c.auth_reached('transaction-guard','uninstall')
        self.assertEqual(c.auth_snapshot()['persistence'],'unknown')

    def test_fresh_recovery_failure_invalidates_previous_current_proof(self):
        c,l=self.grouped();c.auth_observer=Mock();c.auth_observer.close.side_effect=ValueError('lineage')
        with patch.object(m,'relock_account',return_value=credential_proof()), \
             patch.object(m,'recover_group_membership',return_value=membership_proof()):
            with self.assertRaises(ValueError):l.close()
        self.assertEqual(c.auth_snapshot()['recovery']['credential'],'completed')
        with patch.object(m,'relock_account',side_effect=ValueError('PRIVATE')), \
             patch.object(m,'recover_group_membership',side_effect=ValueError('PRIVATE')):
            with self.assertRaises(ValueError):l.close()
        snap=c.auth_snapshot()
        for key in ('credential','group'):
            self.assertEqual(snap['recovery'][key],'unknown');self.assertIsNone(snap[key+'Recovery'])
        c.ops.run.assert_not_called()

    def test_transaction_begin_failure_boundaries_do_not_start_later_steps(self):
        for broken in ('agent','record','lineage','lease','transaction'):
            c=controller(m.AUTH_MODE);c.ledger.value['authentication']['agent']={'identity':identity(80)}
            with self.subTest(broken=broken),patch.object(c,'agent_register') as agent, \
                 patch.object(m,'independent_record') as record,patch.object(m,'AuthObserver') as lineage, \
                 patch.object(m,'CredentialLease') as lease,patch.object(m,'TransactionObserver') as transaction:
                targets=[agent,record,lineage.return_value.start,lease.return_value.open,transaction.return_value.start]
                targets[('agent','record','lineage','lease','transaction').index(broken)].side_effect=ValueError('PRIVATE')
                with self.assertRaises(ValueError):c.transaction_begin('install')
                for later in targets[('agent','record','lineage','lease','transaction').index(broken)+1:]:later.assert_not_called()
            self.assertFalse(c.auth_snapshot()['recovery']['credential']=='completed')

    def test_strict_observation_mode_kind_proof_and_raw_injection(self):
        base=m.auth_runtime_observation(m.AUTH_GROUP_MODE);base['kind']='install'
        for change in (lambda v:v.update(raw='PRIVATE'),lambda v:v['recovery'].update(credential='completed'),
                       lambda v:v.update(groupRecovery=membership_proof()),lambda v:v.update(lastReached='PRIVATE')):
            value=copy.deepcopy(base);change(value)
            with self.assertRaises(ValueError):m.validate_auth_runtime_observation(value)
        value=m.auth_runtime_observation('existing-session');value['recovery']['credential']='attempted'
        with self.assertRaises(ValueError):m.validate_auth_runtime_observation(value)
        value=m.auth_runtime_observation(m.AUTH_MODE);value['recovery']['agent']='completed'
        with self.assertRaises(ValueError):m.validate_auth_runtime_observation(value)

    def test_transaction_begin_failed_initial_scan_never_opens_lease(self):
        c=controller(m.AUTH_MODE)
        with patch.object(c,'agent_register'),patch.object(m,'independent_record'), \
             patch.object(m,'AuthObserver') as observer,patch.object(m,'CredentialLease') as lease:
            observer.return_value.start.side_effect=ValueError('PRIVATE')
            c.ledger.value['authentication']['agent']={'identity':identity(80)}
            with self.assertRaises(ValueError):c.transaction_begin('install')
        lease.assert_not_called();self.assertIsNotNone(c.auth_observer)
        self.assertEqual(c.auth_snapshot()['lastReached'],'lineage-start')


class IndependentDialogueObservationTests(unittest.TestCase):
    def pump(self, chunks, *, echo=0, write=None, select_error=None, stopped=False):
        c=controller(m.AUTH_MODE); c.pty=90; c.auth_reached('transaction-guard','install')
        lease=m.CredentialLease(c,'install');lease.window=90
        lease.user=types.SimpleNamespace(pw_name='runner',pw_gecos='')
        lease.secret=bytearray(b'PRIVATE_PASSWORD');lease.close=Mock()
        if stopped: lease.stop.set()
        writes=[]
        def output(fd,data):
            writes.append(data)
            if write: return write(fd,data)
            return len(data)
        with patch.object(m.select,'select',side_effect=select_error,return_value=([90],[],[])), \
             patch.object(m.os,'read',side_effect=chunks),patch.object(m.os,'write',side_effect=output), \
             patch('termios.tcgetattr',return_value=[0,0,0,echo,0,0,[]]):
            lease.pump()
        return c,lease,writes
    def test_complete_is_independent_from_main_and_detached(self):
        c,lease,writes=self.pump([b'==== AUTHENTICATING FOR org.freedesktop.policykit.exec ====\nAuthenticating as: runner\nPassword: ',b'==== AUTHENTICATION COMPLETE ====\n'])
        c.auth_reached('transaction-observer')
        value=c.auth_snapshot();pump=value['execution']['pump']
        self.assertTrue(lease.authenticated);self.assertEqual(pump['stage'],'completion-observed')
        self.assertTrue(all(pump['observed'].values()));self.assertIsNone(pump['firstFailure'])
        self.assertEqual(value['lastReached'],'transaction-observer')
        self.assertNotIn('PRIVATE',json.dumps(value))
        pump['observed']['ptyBytes']=False
        self.assertTrue(c.auth_snapshot()['execution']['pump']['observed']['ptyBytes'])
    def test_guard_failure_matrix_is_fixed_and_no_secret(self):
        prefix=b'==== AUTHENTICATING FOR org.freedesktop.policykit.exec ====\n'
        cases=[([b''],'pty-eof'),([b'\xff'],'dialogue-encoding'),
            ([b'x'*16385],'dialogue-limit'),([b'AUTHENTICATION FAILED'],'native-authentication-failed'),
            ([b'AUTHENTICATION CANCELED'],'native-authentication-canceled'),
            ([b'Password:'],'action-mismatch'),
            ([prefix+b'Authenticating as: root\nPassword:'],'identity-mismatch'),
            ([prefix+b'Choose identity to authenticate as'],'identity-not-unique'),
            ([b'==== AUTHENTICATION COMPLETE ===='],'completion-unconfirmed')]
        for chunks,code in cases:
            with self.subTest(code=code):
                c,lease,writes=self.pump(chunks);p=c.auth_snapshot()['execution']['pump']
                self.assertEqual(p['firstFailure']['code'],code);self.assertFalse(lease.authenticated)
                self.assertEqual(writes,[]);lease.close.assert_called_once()
                self.assertNotIn('PRIVATE',json.dumps(c.auth_snapshot()))
    def test_io_error_and_short_write_not_completed(self):
        c,lease,_=self.pump([],select_error=OSError('PRIVATE'))
        self.assertEqual(c.auth_snapshot()['execution']['pump']['firstFailure']['code'],'pty-read-failed')
        c,lease,_=self.pump([b'AUTHENTICATING FOR org.freedesktop.policykit.exec\nAuthenticating as: runner\nPassword:'],write=lambda *a:0)
        p=c.auth_snapshot()['execution']['pump'];self.assertEqual(p['firstFailure']['code'],'credential-write-failed')
        self.assertFalse(p['observed']['credentialWriteCompleted']);self.assertFalse(lease.sent)
    def test_echo_timeout_and_identity_write_error(self):
        chunks=[b'AUTHENTICATING FOR org.freedesktop.policykit.exec\nAuthenticating as: runner\nPassword:']
        with patch.object(m.time,'sleep'),patch.object(m.time,'monotonic',side_effect=[0,0,0,3]):
            c,lease,_=self.pump(chunks,echo=8)
        self.assertEqual(c.auth_snapshot()['execution']['pump']['firstFailure']['code'],'echo-unconfirmed')
        def fail(*args): raise OSError('PRIVATE')
        c,lease,_=self.pump([b'AUTHENTICATING FOR org.freedesktop.policykit.exec\n 1.  runner\nChoose identity to authenticate as'],write=fail)
        self.assertEqual(c.auth_snapshot()['execution']['pump']['firstFailure']['code'],'identity-write-failed')
    def test_cleanup_stop_does_not_invent_initial_rejection(self):
        c,lease,_=self.pump([],stopped=True)
        p=c.auth_snapshot()['execution']['pump']
        self.assertEqual(p['stoppedBy'],'controller-cleanup');self.assertIsNone(p['firstFailure'])
        self.assertFalse(p['observed']['credentialWriteCompleted'])
    def test_fixed_failure_is_first_and_diagnostic_failure_does_not_block_close(self):
        c=controller(m.AUTH_MODE);c.auth_reached('transaction-guard','install')
        c.auth_pump('waiting',code='pty-eof');c.auth_pump('stopped',code='unexpected-error')
        self.assertEqual(c.auth_snapshot()['execution']['pump']['firstFailure']['code'],'pty-eof')
        c.lease=Mock();c.ledger.mutate=Mock(side_effect=OSError('PRIVATE'))
        c.fail();c.lease.close.assert_called_once();self.assertTrue(c.recovery_unknown)
    def test_agent_snapshots_are_held_handle_not_continuous_claim(self):
        c=controller(m.AUTH_MODE);c.agent=Mock();c.agent.poll.return_value=None
        c.auth_agent('notify');c.auth_agent('bound')
        a=c.auth_snapshot()['execution']['agent'];self.assertTrue(a['registrationLive']);self.assertTrue(a['latestLive'])
        c.agent.poll.return_value=-15;c.auth_agent('poll')
        a=c.auth_snapshot()['execution']['agent'];self.assertFalse(a['latestLive']);self.assertEqual(a['exitCode'],-15)
        self.assertTrue(a['registrationLive'])
    def test_precleanup_exit_not_overwritten_by_our_term_or_repeated_cleanup(self):
        for before in (None,0,127):
            c=controller(m.AUTH_MODE);c.agent=Mock();c.agent.poll.return_value=before
            c.auth_agent('before-cleanup')
            c.agent.poll.return_value=-15;c.auth_agent('poll');c.auth_agent('before-cleanup')
            a=c.auth_snapshot()['execution']['agent']
            self.assertTrue(a['preCleanupObserved']);self.assertEqual(a['preCleanupLive'],before is None)
            self.assertEqual(a['preCleanupExitCode'],before);self.assertEqual(a['exitCode'],-15)
    def test_all_existing_close_entries_capture_before_cleanup(self):
        import inspect
        for method in (m.CredentialLease.close,m.CredentialLease.close_group,m.Controller.stop_agent):
            source=inspect.getsource(method)
            self.assertIn("auth_note('auth_agent', 'before-cleanup')",source)

    def test_execution_validator_rejects_pollution_and_impossible_flags(self):
        for mutate in (lambda v:v['pump'].update(raw='PRIVATE'),
                       lambda v:v['pump']['observed'].update(authenticationComplete=True),
                       lambda v:v['agent'].update(exitCode=0,latestLive=True),
                       lambda v:v['pump'].update(firstFailure={'stage':'waiting','code':'PRIVATE'}),
                       lambda v:v['agent'].update(registrationLive=True)):
            value=m.auth_execution_observation();mutate(value)
            with self.assertRaises(ValueError):m.validate_auth_execution(value)


class AuthenticationSnapshotConcurrencyTests(unittest.TestCase):
    def test_main_and_pump_snapshots_remain_consistent(self):
        c=controller(m.AUTH_MODE);c.auth_reached('transaction-guard','install');errors=[]
        def pump():
            try:
                c.auth_pump('bytes-observed','ptyBytes')
                c.auth_pump('action-checked','expectedAction')
                c.auth_pump('identity-checked','expectedIdentity')
                c.auth_pump('credential-write','echoDisabled')
                c.auth_pump(flag='credentialWriteCompleted')
                c.auth_pump('completion-observed','authenticationComplete')
            except Exception as e:errors.append(type(e).__name__)
        worker=threading.Thread(target=pump);worker.start()
        for _ in range(100):
            c.auth_reached('transaction-observer');m.validate_auth_runtime_observation(c.auth_snapshot())
        worker.join();self.assertEqual(errors,[]);self.assertFalse(c.recovery_unknown)
        self.assertTrue(c.auth_snapshot()['execution']['pump']['observed']['authenticationComplete'])
    def test_kindless_or_noncredential_dialogue_rejected(self):
        for mode,kind in ((m.AUTH_MODE,None),('existing-session','install')):
            value=m.auth_runtime_observation(mode);value['kind']=kind
            value['execution']['pump']['stage']='waiting'
            with self.assertRaises(ValueError):m.validate_auth_runtime_observation(value)
    def test_activity_deadline_fixed_failure_not_fake_success(self):
        c=controller(m.AUTH_MODE);c.auth_reached('transaction-guard','install');c.pty=90
        lease=m.CredentialLease(c,'install');lease.window=5;lease.user=types.SimpleNamespace(pw_name='runner',pw_gecos='')
        lease.close=Mock()
        lease.pump()
        p=c.auth_snapshot()['execution']['pump'];self.assertEqual(p['firstFailure']['code'],'activity-deadline')
        self.assertEqual(p['stoppedBy'],'deadline');self.assertFalse(lease.authenticated);lease.close.assert_called_once()


class OwnedProcessAgentPreferenceTests(unittest.TestCase):
    def registration(self, mode, *, notify=True, exited=None, bad_identity=False, transaction=False):
        from contextlib import ExitStack
        c=group_controller() if mode==m.AUTH_GROUP_MODE else controller(mode)
        c.auth_reached('transaction-guard','install')
        child=Mock(pid=90);child.poll.return_value=exited;child.wait.return_value=exited
        c.tools['pkttyagent']={'sha256':H}
        c.tun.executable_identity.return_value={'sha256':H}
        row={**identity(90),'ppid':os.getpid(),'uid':1001}
        if bad_identity:row['namespaces']={**row['namespaces'],'net':999}
        c.ops.proc.return_value=row
        with ExitStack() as stack:
            for target,value in (('pty.openpty',(901,902)),('os.pipe',(903,904))):
                stack.enter_context(patch(target,return_value=value))
            stack.enter_context(patch.object(m.pwd,'getpwuid',return_value=types.SimpleNamespace(pw_uid=1001,pw_gid=1001)))
            stack.enter_context(patch.object(m.os,'fchown'));stack.enter_context(patch.object(m.os,'fchmod'))
            close=stack.enter_context(patch.object(m.os,'close'))
            stack.enter_context(patch.object(m.os,'read',return_value=b''))
            stack.enter_context(patch.object(m.select,'select',return_value=([903],[],[]) if notify else ([],[],[])))
            spawn=stack.enter_context(patch.object(m.subprocess,'Popen',return_value=child))
            lease=stack.enter_context(patch.object(m,'CredentialLease'))
            error=None
            try:result=c.transaction_begin('install') if transaction else c.agent_register()
            except Exception as exc:error=exc;result=None
        return c,child,spawn,lease,close,result,error
    def test_complete_argv_modes_exact_subject_and_private_notify(self):
        for mode in (m.AUTH_MODE,m.AUTH_GROUP_MODE,'pkttyagent-observe'):
            with self.subTest(mode=mode):
                c,child,spawn,lease,close,result,error=self.registration(mode)
                self.assertIsNone(error);self.assertTrue(result['registered']);self.assertFalse(result['authenticated'])
                expected=['/usr/bin/setsid','--ctty','/usr/bin/setpriv','--reuid=1001','--regid=1001',
                    '--clear-groups','--bounding-set=-all','--inh-caps=-all','--ambient-caps=-all',
                    '/usr/bin/pkttyagent','--process',str(c.gui['pid'])+','+c.gui['startTime'],'--notify-fd','904']
                if mode!=m.AUTH_GROUP_MODE:expected.append('--fallback')
                self.assertEqual(spawn.call_args.args[0],expected)
                self.assertEqual(spawn.call_args.kwargs,{'env':m.ENV,'stdin':902,'stdout':902,'stderr':902,'pass_fds':(904,)})
                self.assertEqual(c.ledger.load()['authentication']['subject'],{'pid':c.gui['pid'],'startTime':c.gui['startTime']})
                self.assertTrue(c.auth_snapshot()['execution']['agent']['registrationLive'])
                child.kill.assert_not_called();lease.assert_not_called()
                self.assertEqual([call.args[0] for call in close.call_args_list],[902,904,903])
    def test_existing_session_and_duplicate_local_handle_do_not_spawn(self):
        for mode in ('existing-session',m.AUTH_MODE,m.AUTH_GROUP_MODE):
            c=group_controller() if mode==m.AUTH_GROUP_MODE else controller(mode)
            if mode!='existing-session':c.agent=Mock()
            with patch('pty.openpty') as pty_open,patch.object(m.subprocess,'Popen') as spawn:
                with self.assertRaises(ValueError):c.agent_register()
            pty_open.assert_not_called();spawn.assert_not_called()
    def test_existing_subject_registration_rejection_never_opens_lease_or_replaces_agent(self):
        # Polkit's duplicate-subject rejection is an agent exit, not permission
        # to unregister the existing process/session agent or retry registration.
        for mode in (m.AUTH_MODE,m.AUTH_GROUP_MODE):
            c,child,spawn,lease,close,result,error=self.registration(mode,exited=127,transaction=True)
            self.assertIsInstance(error,ValueError);spawn.assert_called_once();lease.assert_not_called()
            child.kill.assert_called_once();child.wait.assert_called_once_with(timeout=5)
            self.assertTrue(c.failed);self.assertNotIn('registered',c.ledger.load()['authentication'])
            c.ops.run.assert_not_called()
    def test_notify_and_exact_birth_identity_failure_still_reap_without_lease(self):
        for mode in (m.AUTH_MODE,m.AUTH_GROUP_MODE):
            for options in ({'notify':False},{'bad_identity':True}):
                with self.subTest(mode=mode,options=options):
                    c,child,spawn,lease,close,result,error=self.registration(mode,transaction=True,**options)
                    self.assertIsInstance(error,ValueError);lease.assert_not_called()
                    child.kill.assert_called_once();child.wait.assert_called_once_with(timeout=5)
                    self.assertTrue(c.failed)
    def test_preference_change_does_not_add_policy_or_budget_actions(self):
        import inspect
        block=inspect.getsource(m.Controller.agent_register)
        self.assertNotIn('unregister',block.split('        try:',1)[1])
        self.assertNotIn('systemctl',block);self.assertNotIn('pkexec',block)
        self.assertEqual(m.GROUP_ACTIVITY_SECONDS,20)
        self.assertEqual(m.GROUP_RECOVERY_RESERVE,70)
        self.assertEqual(m.GROUP_RECOVERY_SERVICE_SECONDS,70)
        for mode in (m.AUTH_MODE,m.AUTH_GROUP_MODE):
            c=group_controller() if mode==m.AUTH_GROUP_MODE else controller(mode)
            self.assertEqual(m.validate_permission(c.launch)['maxLeaseSeconds'],90)

if __name__ == '__main__': unittest.main(verbosity=2)


class TransactionFenceTests(unittest.TestCase):
    def setup_observer(self):
        c = controller(); c.launch['authenticationMode']=m.AUTH_GROUP_MODE; c.auth_note = Mock()
        c.tools = {'pkexec': {'dev':1,'inode':77,'sha256':H}, 'bash': {'dev':1,'inode':88,'sha256':H}}
        c.verify_script = Mock(return_value={'dev':1,'inode':99,'sha256':H,'bytes':123,'stableDoubleRead':True})
        script = '/tmp/qa/home/config/polaris/'+'d'*24+'-polaris-helper-install.sh'
        p = {**identity(60), 'ppid':c.gui['pid'], 'exe':str(Path('/usr/bin/pkexec').resolve())}
        rows = {c.gui['pid']:c.gui,60:p}
        obs = m.TransactionObserver(c,'install'); c.observer=obs
        c.ops.proc.side_effect=lambda pid: rows.get(pid)
        c.tun.transaction_identity.side_effect=lambda pid: {'identity':copy.deepcopy(rows[pid]),
            'uids':(1001,0,0,0) if rows[pid]['uid']==1001 else (0,0,0,0),
            'argv':['/bin/bash',script] if rows[pid]['exe']==str(Path('/bin/bash').resolve()) else ['/usr/bin/pkexec','/bin/bash',script],
            'executable':c.tools['bash' if rows[pid]['exe']==str(Path('/bin/bash').resolve()) else 'pkexec']}
        return c,obs,rows
    def scan(self,obs,rows):
        with patch.object(m.Path,'iterdir',return_value=[Path('/proc',str(pid)) for pid in rows]), \
             patch.object(m,'proc_stat',side_effect=lambda pid:copy.deepcopy(rows.get(pid))): obs.scan()
    def test_initial_pending_root_then_bash_exact_inode_and_script(self):
        c,o,r=self.setup_observer();self.scan(o,r)
        o.credential_fence(time.monotonic()+1,threading.Event())
        r[60]['uid']=0;self.scan(o,r)
        self.assertFalse(o.snapshot()['authorizationObserved'])
        r[60].update(exe=str(Path('/bin/bash').resolve()),exeInode=88);self.scan(o,r)
        self.assertTrue(o.snapshot()['authorizationObserved'])
        self.assertEqual(c.ledger.value['transactionProcesses'][0]['identity']['exeInode'],88)
    def test_root_pkexec_without_prior_rejected(self):
        c,o,r=self.setup_observer();r[60]['uid']=0
        with self.assertRaises(ValueError):self.scan(o,r)
        self.assertEqual(o.error,'pkexec-uid');self.assertFalse(o.pkexec)
    def test_script_replacement_rejected_at_fence_and_shell(self):
        for shell in (False,True):
            c,o,r=self.setup_observer();self.scan(o,r)
            c.verify_script.return_value={**c.verify_script.return_value,'inode':100}
            with self.assertRaises(ValueError):
                if shell:
                    r[60].update(uid=0,exe=str(Path('/bin/bash').resolve()),exeInode=88);self.scan(o,r)
                else:o.credential_fence(time.monotonic()+1,threading.Event())
            self.assertFalse(o.shells)
    def test_unrelated_executable_is_never_read(self):
        c,o,r=self.setup_observer();r[999]={**identity(999),'ppid':1}
        self.scan(o,r)
        self.assertNotIn(999,[v.args[0] for v in c.tun.transaction_identity.call_args_list])
        self.assertNotIn(999,[v.args[0] for v in c.ops.proc.call_args_list])
    def test_related_unstable_candidate_bounded_reads_permanent_failure(self):
        c,o,r=self.setup_observer();c.tun.transaction_identity.side_effect=ValueError('not emitted')
        with self.assertRaises(ValueError):self.scan(o,r)
        self.assertEqual(c.tun.transaction_identity.call_count,3)
        self.assertEqual(o.error,'candidate');self.assertFalse(o.pkexec)
        c.auth_note.assert_called_with('auth_failed','transaction-candidate-unconfirmed')
    def test_fence_no_prior_cancel_deadline_error_no_admission(self):
        for case in ('cancel','deadline','error'):
            c,o,r=self.setup_observer();stop=threading.Event()
            if case=='cancel':stop.set()
            if case=='error':o.error='inventory'
            with self.assertRaises(ValueError):o.credential_fence(time.monotonic()-(1 if case=='deadline' else -1),stop)
            self.assertFalse(o.pkexec)
    def test_fence_rejects_reuse_uid_exec_and_parent_change(self):
        for field,value in [('startTime','9999'),('uid',0),('exeInode',99),('ppid',123)]:
            c,o,r=self.setup_observer();self.scan(o,r);r[60][field]=value
            with self.assertRaises(ValueError):o.credential_fence(time.monotonic()+1,threading.Event())
    def test_fence_wait_holds_no_observer_lock(self):
        c,o,r=self.setup_observer();stop=Mock();stop.is_set.return_value=False
        def wait(seconds):
            self.assertTrue(o.lock.acquire(False));o.lock.release();self.scan(o,r)
        stop.wait.side_effect=wait
        o.credential_fence(time.monotonic()+1,stop)
        stop.wait.assert_called_once()
    def test_ledger_failure_never_publishes_ready_pkexec(self):
        c,o,r=self.setup_observer();c.ledger.mutate=Mock(side_effect=OSError('private'))
        with self.assertRaises(OSError):self.scan(o,r)
        self.assertFalse(o.pkexec);self.assertEqual(o.error,'ledger')
    def test_first_boundary_not_overwritten_by_fence(self):
        c,o,r=self.setup_observer();o.fail_boundary('script');o.fail_boundary('credential-fence')
        self.assertEqual(o.error,'script');c.auth_note.assert_called_once()

class GroupPumpFenceTests(unittest.TestCase):
    def pump(self, fence=None, echo_after=False):
        c=controller();c.launch['authenticationMode']=m.AUTH_GROUP_MODE;c.pty=90;c.auth_note=Mock()
        lease=m.CredentialLease(c,'install');lease.group_deadline=time.monotonic()+90
        lease.user=types.SimpleNamespace(pw_name='runner',pw_gecos='');lease.secret=bytearray(b'test-secret')
        lease.close=Mock();c.observer=Mock();writes=[];order=[]
        def gate(*args):
            order.append('fence')
            if fence=='error':raise ValueError('private')
            if fence=='cancel':lease.stop.set()
        c.observer.credential_fence.side_effect=gate
        prompt=b'==== AUTHENTICATING FOR org.freedesktop.policykit.exec ====\nAuthenticating as: runner\nPassword: '
        import termios
        with patch.object(m.select,'select',return_value=([90],[],[])), \
             patch.object(m.os,'read',side_effect=[prompt,prompt,b'==== AUTHENTICATION COMPLETE ====\n']), \
             patch.object(m.os,'write',side_effect=lambda fd,b:order.append('write') or writes.append(b) or len(b)), \
             patch('termios.tcgetattr',side_effect=[[0,0,0,0,0,0,[]],[0,0,0,termios.ECHO if echo_after else 0,0,0,[]]]):
            lease.pump()
        return lease,writes,order,c
    def test_fence_before_exactly_one_write_repeated_prompt(self):
        lease,writes,order,c=self.pump()
        self.assertEqual(order,['fence','write']);self.assertEqual(writes,[b'test-secret\n'])
        self.assertTrue(lease.authenticated)
        deadline=c.observer.credential_fence.call_args.args[0]
        self.assertEqual(deadline,lease.group_deadline-m.GROUP_RECOVERY_RESERVE)
    def test_error_cancel_or_echo_return_blocks_input_and_closes(self):
        for kwargs in ({'fence':'error'},{'fence':'cancel'},{'echo_after':True}):
            lease,writes,order,c=self.pump(**kwargs)
            self.assertEqual(writes,[]);self.assertFalse(lease.authenticated);lease.close.assert_called_once()
    def test_existing_password_only_pump_does_not_call_fence(self):
        # Existing guarded password-only unit test exercises the original mode.
        lease,writes=AdditionalContracts().pump([b'==== AUTHENTICATING FOR org.freedesktop.policykit.exec ====\nAuthenticating as: runner\nPassword: ',b'==== AUTHENTICATION COMPLETE ====\n'])
        self.assertFalse(lease.grouped);self.assertEqual(len(writes),1)

class RelatedInventoryTests(unittest.TestCase):
    def test_auth_scan_does_not_read_unrelated_executable(self):
        c=controller();agent=identity(90);child={**identity(91,0),'ppid':90};other={**identity(999),'ppid':1}
        rows={90:agent,91:child,999:other};c.ledger.value['authentication']['agent']={'identity':agent}
        c.ops.proc.side_effect=lambda pid: (_ for _ in ()).throw(PermissionError()) if pid==999 else rows.get(pid)
        o=m.AuthObserver(c,'install')
        with patch.object(m.Path,'iterdir',return_value=[Path('/proc',str(x)) for x in rows]), \
             patch.object(m,'proc_stat',side_effect=lambda pid:rows.get(pid)),patch.object(m,'write_recovery_snapshot'):
            o.scan()
        self.assertEqual(len(o.rows),2)
    def test_auth_unknown_related_identity_still_rejected(self):
        c=controller();agent=identity(90);child={**identity(91,0),'ppid':90}
        rows={90:agent,91:child};c.ledger.value['authentication']['agent']={'identity':agent}
        c.ops.proc.side_effect=lambda pid: (_ for _ in ()).throw(PermissionError()) if pid==91 else rows.get(pid)
        o=m.AuthObserver(c,'install')
        with patch.object(m.Path,'iterdir',return_value=[Path('/proc',str(x)) for x in rows]), \
             patch.object(m,'proc_stat',side_effect=lambda pid:rows.get(pid)),patch.object(m,'write_recovery_snapshot'):
            with self.assertRaises(PermissionError):o.scan()
    def test_child_before_parent_inventory_updates_exec_first(self):
        t=TransactionFenceTests();c,o,r=t.setup_observer();t.scan(o,r)
        r[60].update(uid=0,exe=str(Path('/bin/bash').resolve()),exeInode=88)
        child={**identity(61,0),'ppid':60,'exe':'/root/verified-child','exeInode':101}
        r[61]=child;r={61:r[61],60:r[60],c.gui['pid']:c.gui}
        original=c.tun.transaction_identity.side_effect
        c.tun.transaction_identity.side_effect=lambda pid: {'identity':child,'uids':(0,)*4,'argv':['child'],'executable':{'dev':1,'inode':101,'sha256':H}} if pid==61 else original(pid)
        t.scan(o,r);self.assertIn(m.birth(child),o.rows);self.assertTrue(o.shells)
    def test_fence_contended_lock_deadline_is_bounded(self):
        c,o,r=TransactionFenceTests().setup_observer();o.lock.acquire()
        try:
            before=time.monotonic()
            with self.assertRaises(ValueError):o.credential_fence(before+0.02,threading.Event())
            self.assertLess(time.monotonic()-before,0.5)
        finally:o.lock.release()

class RealProducerScriptTests(unittest.TestCase):
    def fixture(self, kind):
        return (HERE/'fixtures'/('linux-helper-'+kind+'.sh')).read_bytes()
    def test_complete_actual_producer_install_and_uninstall_are_accepted(self):
        for kind in ('install','uninstall'):
            data=self.fixture(kind)
            self.assertTrue(data.startswith(b'#!/bin/sh\nset -eu\n'))
            with tempfile.TemporaryDirectory() as home:
                base=Path(home);base.chmod(0o700);directory=base/'config';directory.mkdir(mode=0o700)
                path=directory/('a'*24+'-polaris-helper-'+kind+'.sh');path.write_bytes(data);path.chmod(0o700)
                c=controller();c.launch.update(guiUid=os.getuid(),guiHome=home,helperScriptDir=str(directory))
                proof=c.verify_script(path,kind)
                self.assertEqual(proof['bytes'],len(data));self.assertEqual(proof['sha256'],m.sha(data))
                self.assertEqual(proof['inode'],path.stat().st_ino);self.assertTrue(proof['stableDoubleRead'])
                self.assertNotIn('eeeeeeee',json.dumps(proof))
    def test_old_bash_other_shebang_crlf_and_wrong_kind_still_rejected(self):
        data=self.fixture('install')
        with tempfile.TemporaryDirectory() as home:
            base=Path(home);base.chmod(0o700);path=base/('a'*24+'-polaris-helper-install.sh')
            c=controller();c.launch.update(guiUid=os.getuid(),guiHome=home,helperScriptDir=home)
            for value in (data.replace(b'#!/bin/sh',b'#!/bin/bash',1),data.replace(b'#!/bin/sh',b'#!/usr/bin/env sh',1),data.replace(b'\n',b'\r\n'),b'\xef\xbb\xbf'+data):
                path.write_bytes(value);path.chmod(0o700)
                with self.assertRaises(ValueError):c.verify_script(path,'install')
            path.write_bytes(data)
            with self.assertRaises(ValueError):c.verify_script(path,'uninstall')
    def test_unlinked_script_held_fd_is_not_a_live_private_path_proof(self):
        data=self.fixture('install')
        with tempfile.TemporaryDirectory() as home:
            base=Path(home);base.chmod(0o700);path=base/('a'*24+'-polaris-helper-install.sh');path.write_bytes(data);path.chmod(0o700)
            c=controller();c.launch.update(guiUid=os.getuid(),guiHome=home,helperScriptDir=home)
            with path.open('rb') as held:
                path.unlink();self.assertEqual(held.read(),data)
                with self.assertRaises(FileNotFoundError):c.verify_script(path,'install')
    def test_actual_fixture_shared_dynamic_fields_remain_escaped(self):
        for kind in ('install','uninstall'):
            text=self.fixture(kind).decode()
            self.assertIn("'/tmp/producer fixture/用户'\\''s stage/polaris-helper'",text)
        self.assertIn("'u1001'",self.fixture('install').decode())

class PreParentStopLineageTests(unittest.TestCase):
    def observer(self):
        c=controller();agent=identity(90);child={**identity(91,0),'ppid':90}
        c.ledger.value['authentication']['agent']={'identity':copy.deepcopy(agent)}
        rows={90:agent,91:child};c.ops.proc.side_effect=lambda pid:copy.deepcopy(rows.get(pid))
        o=m.AuthObserver(c,'install');return c,o,rows
    def test_existing_child_snapshot_is_durable_while_parent_alive(self):
        c,o,rows=self.observer();events=[]
        with patch.object(m.Path,'iterdir',return_value=[Path('/proc/90'),Path('/proc/91')]), \
             patch.object(m,'proc_stat',side_effect=lambda pid:rows.get(pid)), \
             patch.object(m,'write_recovery_snapshot',side_effect=lambda *a:events.append(copy.deepcopy(a[2]))):
            o.thread.start();o.prepare_parent_stop();o.thread.join(timeout=1)
        self.assertTrue(o.stop.is_set());self.assertEqual(len(events),1)
        self.assertEqual({x['pid'] for x in events[0]['processes']},{90,91});self.assertIsNone(o.error)
    def test_snapshot_error_timeout_or_persistence_never_skips_independent_recovery(self):
        for error in (ValueError('PRIVATE'),TimeoutError('PRIVATE'),OSError('PRIVATE')):
            c,l=GroupLeaseRecoveryTests().lease();events=[];c.auth_observer=Mock();observer=c.auth_observer
            def snapshot(*args):
                self.assertTrue(l.close_lock.acquire(False));l.close_lock.release()
                events.append('snapshot');raise error
            c.auth_observer.prepare_parent_stop.side_effect=snapshot
            c.auth_observer.close.return_value={'ownedBirths':[],'unknownAdoptedChildren':[],'forced':False}
            c.fence_registered_agent=Mock(side_effect=lambda:events.append('fence'))
            with patch.object(m,'relock_account',side_effect=lambda *a:events.append('relock') or credential_proof()), \
                 patch.object(m,'recover_group_membership',side_effect=lambda *a:events.append('remove') or membership_proof()):
                with self.assertRaises(ValueError):l.close()
            self.assertEqual(events,['snapshot','fence','relock','remove'])
            observer.close.assert_called_once();c.ops.run.assert_not_called()
            self.assertFalse(l.closed);self.assertIn('authentication-lineage-stop-unproved',c.recovery_unknown)
    def test_unknown_later_fork_not_authorized_by_prior_snapshot(self):
        c,o,rows=self.observer()
        with patch.object(m.Path,'iterdir',return_value=[Path('/proc/90'),Path('/proc/91')]), \
             patch.object(m,'proc_stat',side_effect=lambda pid:rows.get(pid)),patch.object(m,'write_recovery_snapshot'):
            o.thread.start();o.prepare_parent_stop();o.thread.join(timeout=1)
        orphan={**identity(92,0),'ppid':os.getpid()};rows.clear();rows[92]=orphan
        with patch.object(m.Path,'iterdir',return_value=[Path('/proc/92')]), \
             patch.object(m,'proc_stat',side_effect=lambda pid:rows.get(pid)),patch.object(m,'write_recovery_snapshot'), \
             patch.object(m,'stop_recorded',return_value=False) as stop:
            with self.assertRaisesRegex(ValueError,'unobserved adopted child'):o.close()
        self.assertNotIn(92,[call.args[2]['pid'] for call in stop.call_args_list])
    def test_pre_stop_unstarted_or_unacknowledged_thread_fails_closed(self):
        for started in (False,True):
            c,o,r=self.observer()
            if started:o.thread=Mock();o.thread.ident=1
            with self.assertRaises(ValueError):o.prepare_parent_stop(time.monotonic()+0.01)
            self.assertEqual(o.error,'pre-stop-observation-incomplete')
    def test_successful_ack_is_idempotent_but_never_clears_prior_failure(self):
        c,o,r=self.observer();o.thread=Mock();o.thread.ident=1;o.parent_stop_ack.set()
        o.prepare_parent_stop();o.prepare_parent_stop();self.assertIsNone(o.error)
        o.error='pre-stop-observation-incomplete'
        with self.assertRaises(ValueError):o.prepare_parent_stop()
    def test_real_hung_snapshot_cannot_block_fence_relock_remove(self):
        c,l=GroupLeaseRecoveryTests().lease();agent=identity(90)
        c.ledger.value['authentication']['agent']={'identity':agent}
        o=m.AuthObserver(c,'install');c.auth_observer=o
        entered=threading.Event();release=threading.Event();events=[];late=[]
        def blocked():
            entered.set();release.wait();late.append('observation-only')
        o.scan=blocked
        c.fence_registered_agent=Mock(side_effect=lambda:events.append('fence'))
        o.thread.start();self.assertTrue(entered.wait(1))
        l.group_deadline=time.monotonic()+0.03
        try:
            with patch.object(m,'relock_account',side_effect=lambda *a:events.append('relock') or credential_proof()), \
                 patch.object(m,'recover_group_membership',side_effect=lambda *a:events.append('remove') or membership_proof()), \
                 patch.object(o.thread,'join',return_value=None):
                before=time.monotonic()
                with self.assertRaises(ValueError):l.close()
                self.assertLess(time.monotonic()-before,0.5)
            self.assertFalse(release.is_set());self.assertEqual(events,['fence','relock','remove'])
            self.assertFalse(l.closed);self.assertIsNone(c.ledger.value['authentication']['lease'].get('state'))
            self.assertTrue(c.recovery_unknown);c.ops.run.assert_not_called()
            old_unknown=list(c.recovery_unknown)
        finally:release.set();o.thread.join(timeout=1)
        self.assertEqual(late,['observation-only']);self.assertEqual(c.recovery_unknown,old_unknown)
        self.assertEqual(o.error,'pre-stop-observation-incomplete');self.assertFalse(l.closed)
    def test_request_during_old_tick_needs_next_durable_scan(self):
        c,o,r=self.observer();entered=threading.Event();release=threading.Event();calls=[]
        def scan():
            calls.append(len(calls)+1)
            if len(calls)==1:entered.set();release.wait()
        o.scan=scan;o.thread.start();self.assertTrue(entered.wait(1))
        o.parent_stop_requested.set();self.assertFalse(o.parent_stop_ack.is_set());release.set()
        self.assertTrue(o.parent_stop_ack.wait(1));o.thread.join(timeout=1)
        self.assertEqual(calls,[1,2])


class FinalSnapshotParentTests(unittest.TestCase):
    def test_missing_reused_or_exited_parent_cannot_ack_success(self):
        for mode in ('missing','reused','zombie'):
            c,o,rows=PreParentStopLineageTests().observer()
            if mode=='missing': rows.pop(90)
            elif mode=='reused': rows[90]['startTime']='999'
            else: rows[90]['state']='Z'
            o.scan=Mock();o.parent_stop_requested.set();o.thread.start()
            try:
                with self.assertRaises(ValueError):o.prepare_parent_stop(time.monotonic()+1)
            finally:o.thread.join(timeout=1)
            self.assertIsNotNone(o.error);o.scan.assert_not_called()
    def test_parent_disappears_during_final_snapshot_is_unknown(self):
        c,o,rows=PreParentStopLineageTests().observer()
        o.scan=Mock(side_effect=lambda:rows.pop(90));o.parent_stop_requested.set();o.thread.start()
        try:
            with self.assertRaises(ValueError):o.prepare_parent_stop(time.monotonic()+1)
        finally:o.thread.join(timeout=1)
        self.assertIsNotNone(o.error);o.scan.assert_called_once()


class CandidateReconciliationTests(unittest.TestCase):
    setup_observer = TransactionFenceTests.setup_observer
    scan = TransactionFenceTests.scan

    def scenario(self, bad=None):
        c,o,r=self.setup_observer(); self.scan(o,r)
        r[61]={**identity(61,uid=0),'ppid':60,'exe':'/fixed-child'}
        original=c.tun.transaction_identity.side_effect; changed=[False]
        def observation(pid):
            if pid==61:
                if not changed[0]:
                    changed[0]=True
                    r[60].update(uid=0,exe=str(Path('/bin/bash').resolve()),exeInode=88)
                    if bad=='reuse':r[60]['startTime']='9999'
                    if bad=='namespace':r[60]['namespaces']={**r[60]['namespaces'],'mnt':99}
                    if bad=='script':c.verify_script.return_value={**c.verify_script.return_value,'inode':999}
                    if bad=='gone':r.pop(60)
                return {'identity':copy.deepcopy(r[61]),'uids':(0,0,0,0),'argv':['/fixed-child'],
                        'executable':{'dev':1,'inode':161,'sha256':H}}
            value=original(pid)
            if bad=='argv' and changed[0]:value['argv']=['/bin/bash','/wrong']
            return value
        c.tun.transaction_identity.side_effect=observation
        c.tun.transaction_identity.reset_mock()
        return c,o,r

    def test_parent_same_birth_exec_during_child_read_revalidates_whole_chain(self):
        c,o,r=self.scenario();self.scan(o,r)
        self.assertTrue(o.snapshot()['authorizationObserved'])
        self.assertIn(m.birth(r[61]),o.rows)
        self.assertEqual([x.args[0] for x in c.tun.transaction_identity.call_args_list],[60,61,60,61])
        recorded=c.ledger.value['transactionProcesses']
        self.assertEqual(next(x['identity']['exeInode'] for x in recorded if x['identity']['pid']==60),88)

    def test_changed_parent_never_relaxes_prior_script_argv_namespace_or_birth(self):
        for bad in ('reuse','namespace','script','gone','argv'):
            with self.subTest(bad=bad):
                c,o,r=self.scenario(bad)
                with self.assertRaises(ValueError):self.scan(o,r)
                self.assertNotIn(m.birth(r[61]),o.rows)
                self.assertIsNotNone(o.error)

    def test_unknown_related_gone_or_zombie_still_permanent_failure(self):
        for state in (None,'Z'):
            c,o,r=self.setup_observer()
            error=vm_tun.TransactionIdentityError('identity-before',ValueError('private-message'))
            c.tun.transaction_identity.side_effect=error
            with self.assertRaises(ValueError):self.scan(o,r)
            self.assertFalse(o.rows);self.assertEqual(o.error,'candidate')
            diagnostic=next(x.args[1] for x in c.auth_note.call_args_list if x.args[0]=='auth_candidate_failure')
            self.assertEqual(diagnostic['phase'],'identity-before')
            self.assertNotIn('private-message',json.dumps(diagnostic))
            m.validate_candidate_failure(diagnostic)

    def test_diagnostic_distinguishes_parent_reuse_from_candidate_io(self):
        c,o,r=self.scenario('reuse')
        with self.assertRaises(ValueError):self.scan(o,r)
        d=next(x.args[1] for x in c.auth_note.call_args_list if x.args[0]=='auth_candidate_failure')
        self.assertEqual((d['phase'],d['inventory']['pid'],d['parent']['pid']),('owned-parent',61,60))
        c,o,r=self.setup_observer()
        c.tun.transaction_identity.side_effect=vm_tun.TransactionIdentityError('executable-open',PermissionError(13,'private-path'))
        with self.assertRaises(ValueError):self.scan(o,r)
        d=next(x.args[1] for x in c.auth_note.call_args_list if x.args[0]=='auth_candidate_failure')
        self.assertEqual((d['phase'],d['error']),('executable-open','eacces'))

    def test_strict_candidate_schema_and_first_diagnostic_is_detached(self):
        c=controller(m.AUTH_MODE);c.auth_runtime=m.auth_runtime_observation(m.AUTH_MODE)
        c.auth_runtime['kind']='install';c.auth_runtime_lock=threading.RLock();c.auth_checkpoint=Mock()
        d={'schema':'polaris-transaction-candidate-failure-v1','phase':'owned-parent',
           'inventory':{'pid':61,'startTime':'1234','ppid':60},'relation':'owned-child',
           'parent':{'pid':60,'startTime':'1234'},'outcome':'identity-mismatch','error':'contract'}
        c.auth_candidate_failure(d);d['inventory']['pid']=999
        self.assertEqual(c.auth_snapshot()['candidateFailure']['inventory']['pid'],61)
        for key,value in (('phase','raw-secret'),('error','private-path'),('inventory',{'pid':True,'startTime':'1','ppid':2})):
            bad=copy.deepcopy(c.auth_snapshot()['candidateFailure']);bad[key]=value
            with self.assertRaises(ValueError):m.validate_candidate_failure(bad)

    def test_candidate_shared_budget_and_fence_deadline_do_not_observe(self):
        c,o,r=self.setup_observer();c.tun.transaction_identity.reset_mock()
        with self.assertRaises(ValueError):o.candidate(60,r[60],budget=[0])
        c.tun.transaction_identity.assert_not_called()
        with self.assertRaises(ValueError):o.credential_fence(time.monotonic()-1,threading.Event())
        c.tun.transaction_identity.assert_not_called()


class CandidateExtraBoundariesTests(unittest.TestCase):
    setup_observer = TransactionFenceTests.setup_observer
    scan = TransactionFenceTests.scan
    scenario = CandidateReconciliationTests.scenario
    def test_parent_reparent_is_not_a_legal_exec_transition(self):
        c,o,r=self.scenario();original=c.ops.proc.side_effect
        def read(pid):
            value=original(pid)
            if pid==60 and value and value['uid']==0:return {**value,'ppid':999}
            return value
        c.ops.proc.side_effect=read
        with self.assertRaises(ValueError):self.scan(o,r)
        self.assertNotIn(m.birth(r[61]),o.rows)
    def test_parent_transition_ledger_failure_never_admits_child(self):
        c,o,r=self.scenario();original=c.ledger.mutate;calls=[0]
        def mutate(fn):
            calls[0]+=1
            if calls[0]==2:raise OSError('not emitted')
            return original(fn)
        c.ledger.mutate=mutate
        with self.assertRaises(OSError):self.scan(o,r)
        self.assertNotIn(m.birth(r[61]),o.rows)
        self.assertEqual(o.error,'ledger')


class OwnedTerminalReconciliationTests(unittest.TestCase):
    setup_observer = TransactionFenceTests.setup_observer
    scan = TransactionFenceTests.scan
    def test_only_durable_strong_owned_birth_can_be_terminal(self):
        for known in (False,True):
            c,o,r=self.setup_observer()
            if known:self.scan(o,r)
            c.tun.transaction_identity.side_effect=vm_tun.TransactionIdentityError('identity-before',ValueError('unconfirmed'))
            c.tun.terminal_identity.return_value={'pid':60,'startTime':r[60]['startTime'],'state':'absent','scope':'sampled-terminal-not-execution-identity'}
            if known:
                self.scan(o,r)
                self.assertIn(m.birth(r[60]),o.terminals)
                self.assertFalse(o.snapshot()['authorizationObserved'])
                with self.assertRaises(ValueError):
                    o.thread=Mock();o.thread.is_alive.return_value=False;o.finish()
            else:
                with self.assertRaises(ValueError):self.scan(o,r)
                c.tun.terminal_identity.assert_not_called()
    def test_owned_permission_or_live_reuse_is_never_reconciled(self):
        for error in (vm_tun.TransactionIdentityError('identity-before',PermissionError(13,'hidden')),
                      vm_tun.TransactionIdentityError('snapshot-consistency',ValueError('changed'))):
            c,o,r=self.setup_observer();self.scan(o,r)
            c.tun.transaction_identity.side_effect=error
            with self.assertRaises(ValueError):self.scan(o,r)
            c.tun.terminal_identity.assert_not_called()
        c,o,r=self.setup_observer();self.scan(o,r)
        c.tun.transaction_identity.side_effect=vm_tun.TransactionIdentityError('identity-before',ValueError('gone'))
        c.tun.terminal_identity.side_effect=ValueError('reused')
        with self.assertRaises(ValueError):self.scan(o,r)
        self.assertFalse(o.terminals)
    def test_terminal_ledger_failure_remains_failure(self):
        c,o,r=self.setup_observer();self.scan(o,r)
        c.tun.transaction_identity.side_effect=vm_tun.TransactionIdentityError('identity-before',ValueError('gone'))
        c.tun.terminal_identity.return_value={'pid':60,'startTime':r[60]['startTime'],'state':'zombie','scope':'sampled-terminal-not-execution-identity'}
        c.ledger.mutate=Mock(side_effect=OSError('disk'))
        with self.assertRaises(OSError):self.scan(o,r)
        self.assertFalse(o.terminals)
